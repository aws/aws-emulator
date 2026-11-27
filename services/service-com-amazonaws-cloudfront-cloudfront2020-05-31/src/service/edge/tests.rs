// SPDX-License-Identifier: CC0-1.0

//! The pure rules this module owns that a wire test cannot vary cheaply: how a URI is
//! split into an addressing, and how an origin request URI is built. The served
//! behaviour is pinned end to end in `tests/all/edge_data_plane.rs`.

use super::*;
use crate::ops::types::CustomOriginConfig;

#[test]
fn the_origins_own_response_headers_are_served_except_the_listed_ones() {
    // Rows differ along WHY a header is or is not relayed: a header the origin stated
    // about the object, each class the exclusion list carries (a hop-by-hop header, the
    // three that describe the bytes being served, the two the edge states itself), the
    // same names spelled in another case, and a name that merely resembles one of them.
    let cases: [(&str, &str, bool); 14] = [
        ("the object's entity tag", "etag", true),
        ("a cache directive", "cache-control", true),
        ("user metadata", "x-amz-meta-note", true),
        ("a cookie the origin set", "set-cookie", true),
        ("a header name in upper case", "ETag", true),
        ("a hop-by-hop header", "connection", false),
        ("another hop-by-hop header", "transfer-encoding", false),
        ("a hop-by-hop header in mixed case", "Keep-Alive", false),
        ("the length of the bytes served", "content-length", false),
        ("the media type the edge restates", "content-type", false),
        ("the coding the edge restates", "content-encoding", false),
        ("the cache outcome the edge states", X_CACHE, false),
        ("the Via the edge states for the leg it serves", VIA, false),
        ("a name that merely resembles one", "x-connection-note", true),
    ];
    for (case, name, expected) in cases {
        let actual = is_served_from_origin(name);
        assert_eq!(
            actual, expected,
            "case {case:?} ({name:?}): expected is_served_from_origin == {expected}, got \
                 {actual}"
        );
    }
}

#[test]
fn a_rewritten_uri_keeps_the_addressing_the_viewer_dialled() {
    // Rows differ along the addressing the request arrived under (a distribution's own
    // host in origin form, the same absolute, the reserved data-plane path), along what
    // the function returned for the URI (a path, a path with no leading slash) and along
    // whether it returned a query string.
    // (case, the request's URI, the viewer path within the distribution, the URI the
    //  function returned, the query it returned, the URI the edge then serves)
    type Row = (&'static str, &'static str, &'static str, &'static str, &'static str, &'static str);
    let cases: [Row; 5] = [
        (
            "a host-addressed request",
            "/original.txt",
            "/original.txt",
            "/rewritten.txt",
            "",
            "/rewritten.txt",
        ),
        (
            "an absolute request URI",
            "http://d111.cloudfront.net/original.txt",
            "/original.txt",
            "/rewritten.txt",
            "",
            "http://d111.cloudfront.net/rewritten.txt",
        ),
        (
            "the reserved data-plane path",
            "/_aws/cloudfront/E123/original.txt",
            "/original.txt",
            "/rewritten.txt",
            "",
            "/_aws/cloudfront/E123/rewritten.txt",
        ),
        (
            "a URI the function returned without a leading slash",
            "/original.txt",
            "/original.txt",
            "rewritten.txt",
            "",
            "/rewritten.txt",
        ),
        (
            "a query string the function returned",
            "/original.txt?a=1",
            "/original.txt",
            "/rewritten.txt",
            "b=2",
            "/rewritten.txt?b=2",
        ),
    ];
    for (case, uri, viewer_path, rewritten, query, expected) in cases {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri(uri.to_string()).expect("a valid URI");
        req.headers_mut()
            .insert("host".to_string(), "d111.cloudfront.net".to_string());
        let rewrite = functions::Request {
            uri: rewritten.to_string(),
            query: query.to_string(),
            headers: vec![("host".to_string(), "d111.cloudfront.net".to_string())],
        };
        let request =
            rewritten_request(&req, viewer_path, &rewrite).expect("the rewrite renders");
        assert_eq!(
            request.uri(),
            expected,
            "case {case:?}: expected the edge to serve {expected:?}, got {:?}",
            request.uri()
        );
    }
}

#[test]
fn a_uri_is_split_into_its_path_and_its_query() {
    // Rows differ along the form of the URI (origin-form, absolute), whether it
    // carries a query, and whether it carries a path at all.
    let cases: [(&str, &str, &str); 6] = [
        ("/", "/", ""),
        ("/opt/thing", "/opt/thing", ""),
        ("/opt/thing?a=1&b=2", "/opt/thing", "a=1&b=2"),
        ("https://d1.cloudfront.net/opt", "/opt", ""),
        ("https://d1.cloudfront.net/opt?a=1", "/opt", "a=1"),
        ("https://d1.cloudfront.net", "/", ""),
    ];
    for (uri, path, query) in cases {
        let (actual_path, actual_query) = (path_of(uri), query_of(uri));
        assert_eq!(
            (actual_path, actual_query),
            (path, query),
            "case {uri:?}: expected the path {path:?} and the query {query:?}, got \
                 {actual_path:?} and {actual_query:?}"
        );
    }
}

#[test]
fn the_data_plane_path_prefix_names_the_distribution_before_the_viewer_path() {
    // Rows differ along what follows the prefix (an id with a viewer path, an id
    // alone), along the spelling of the prefix itself, and along paths that carry no
    // prefix at all.
    let cases: [(&str, Option<(&str, &str)>); 6] = [
        ("/_aws/cloudfront/E123/opt/x", Some(("E123", "/opt/x"))),
        ("/_aws/cloudfront/E123", Some(("E123", "/"))),
        ("/_aws/cloudfront/E123/", Some(("E123", "/"))),
        ("/_aws/cloudfront", None),
        ("/_aws/execute-api/abc/stage", None),
        ("/2020-05-31/distribution", None),
    ];
    for (path, expected) in cases {
        let actual = split_edge_prefix(path);
        assert_eq!(
            actual, expected,
            "case {path:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[test]
fn an_origin_request_uri_carries_the_origins_scheme_port_and_path() {
    // Rows differ along the origin protocol policy, the ports, the origin path and
    // the viewer query — everything the rule generalises over.
    let custom = |policy: &str, http_port: i32, https_port: i32| CustomOriginConfig {
        http_port,
        https_port,
        origin_protocol_policy: policy.to_string(),
        ..CustomOriginConfig::default()
    };
    let origin = |config: Option<CustomOriginConfig>, origin_path: Option<&str>| Origin {
        id: "primary".to_string(),
        domain_name: "origin.example.com".to_string(),
        origin_path: origin_path.map(str::to_string),
        custom_origin_config: config,
        ..Origin::default()
    };
    // (case, the origin, the scheme the viewer dialled, the viewer path, the query, the
    // origin URI)
    let cases: [(&str, Origin, &str, &str, &str, &str); 9] = [
        (
            "an https-only origin on the default port",
            origin(Some(custom("https-only", 80, 443)), None),
            "https",
            "/index.html",
            "",
            "https://origin.example.com/index.html",
        ),
        (
            "an http-only origin on the default port",
            origin(Some(custom("http-only", 80, 443)), None),
            "https",
            "/index.html",
            "",
            "http://origin.example.com/index.html",
        ),
        (
            "a match-viewer origin for a viewer that dialled plaintext",
            origin(Some(custom("match-viewer", 80, 443)), None),
            "http",
            "/index.html",
            "",
            "http://origin.example.com/index.html",
        ),
        (
            "a match-viewer origin for a viewer that dialled TLS",
            origin(Some(custom("match-viewer", 80, 443)), None),
            "https",
            "/index.html",
            "",
            "https://origin.example.com/index.html",
        ),
        (
            "a match-viewer origin on an http port of its own",
            origin(Some(custom("match-viewer", 8080, 443)), None),
            "http",
            "/index.html",
            "",
            "http://origin.example.com:8080/index.html",
        ),
        (
            "an https origin on a port of its own",
            origin(Some(custom("https-only", 80, 8443)), None),
            "https",
            "/index.html",
            "",
            "https://origin.example.com:8443/index.html",
        ),
        (
            "an origin path the viewer path is appended to",
            origin(Some(custom("https-only", 80, 443)), Some("/prefix")),
            "https",
            "/index.html",
            "",
            "https://origin.example.com/prefix/index.html",
        ),
        (
            "an S3 origin, over https, carrying the viewer query",
            origin(None, None),
            "http",
            "/key",
            "a=1",
            "https://origin.example.com/key?a=1",
        ),
        (
            // A domain name that already states a port is the whole address, so the
            // configured port is not written after it.
            "an origin whose domain name states the address it is served at",
            Origin {
                domain_name: "127.0.0.1:34567".to_string(),
                ..origin(Some(custom("http-only", 8080, 443)), None)
            },
            "https",
            "/index.html",
            "",
            "http://127.0.0.1:34567/index.html",
        ),
    ];
    for (case, origin, viewer_scheme, viewer_path, query, expected) in cases {
        let actual = origin_uri(&origin, None, viewer_scheme, viewer_path, query);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the origin URI {expected:?}, got {actual:?}"
        );
    }
}

/// A VPC ORIGIN states the protocol policy and the ports of its fetch on the VPC origin
/// resource the origin NAMES, so an origin carrying no `CustomOriginConfig` at all is
/// addressed by that endpoint configuration rather than as an Amazon S3 origin.
#[test]
fn a_vpc_origins_request_uri_is_addressed_by_its_endpoint_configuration() {
    let endpoint = |policy: &str, http_port: i32, https_port: i32| VpcOriginEndpointConfig {
        name: "endpoint".to_string(),
        arn: "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/edge/0"
            .to_string(),
        http_port,
        https_port,
        origin_protocol_policy: policy.to_string(),
        ..VpcOriginEndpointConfig::default()
    };
    let origin = |domain: &str| Origin {
        id: "vpc".to_string(),
        domain_name: domain.to_string(),
        vpc_origin_config: Some(crate::ops::types::VpcOriginConfig {
            vpc_origin_id: "vo_0123456789abcdef".to_string(),
            ..crate::ops::types::VpcOriginConfig::default()
        }),
        ..Origin::default()
    };
    // (case, the origin, the endpoint configuration of the VPC origin it names, the
    // scheme the viewer dialled, the origin URI)
    let cases: [(&str, Origin, VpcOriginEndpointConfig, &str, &str); 5] = [
        (
            "http-only on the default port, to a host",
            origin("origin.example.com"),
            endpoint("http-only", 80, 443),
            "https",
            "http://origin.example.com/index.html",
        ),
        (
            "https-only on a port of its own",
            origin("origin.example.com"),
            endpoint("https-only", 80, 8443),
            "http",
            "https://origin.example.com:8443/index.html",
        ),
        (
            "match-viewer for a viewer that dialled plaintext",
            origin("origin.example.com"),
            endpoint("match-viewer", 8080, 8443),
            "http",
            "http://origin.example.com:8080/index.html",
        ),
        (
            "match-viewer for a viewer that dialled TLS",
            origin("origin.example.com"),
            endpoint("match-viewer", 8080, 8443),
            "https",
            "https://origin.example.com:8443/index.html",
        ),
        (
            // The address the load balancer behind a VPC origin publishes names its own
            // port, and that address is where the fetch goes.
            "http-only, to the address the endpoint is served at",
            origin("127.0.0.1:34567"),
            endpoint("http-only", 80, 443),
            "https",
            "http://127.0.0.1:34567/index.html",
        ),
    ];
    for (case, origin, endpoint, viewer_scheme, expected) in cases {
        let actual = origin_uri(&origin, Some(&endpoint), viewer_scheme, "/index.html", "");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the origin URI {expected:?}, got {actual:?}"
        );
    }
}

#[test]
fn an_origin_addressed_over_a_scheme_it_does_not_serve_cannot_be_connected_to() {
    // Rows differ along the scheme the origin request is addressed over and along the
    // KIND of host it names: an AWS service endpoint this emulator serves (which is
    // served over TLS alone), a bucket's own virtual-hosted endpoint, a bucket's WEBSITE
    // endpoint (which is the other way round: plaintext and no TLS), a per-resource
    // listener of this emulator (which negotiates its encryption in band), and a host
    // nothing here serves.
    let cases: [(&str, &str, bool); 13] = [
        (
            "an api stage over TLS",
            "https://abcdef1234.execute-api.us-east-1.amazonaws.com/live/echo",
            true,
        ),
        (
            "an api stage over plaintext",
            "http://abcdef1234.execute-api.us-east-1.amazonaws.com/live/echo",
            false,
        ),
        (
            "a bucket endpoint over TLS",
            "https://shelf.s3.us-east-1.amazonaws.com/key",
            true,
        ),
        (
            "a bucket endpoint over plaintext",
            "http://shelf.s3.us-east-1.amazonaws.com/key",
            false,
        ),
        (
            "a bucket's website endpoint over plaintext",
            "http://shelf.s3-website-us-east-1.amazonaws.com/index.html",
            true,
        ),
        (
            "a bucket's website endpoint over TLS, which it does not serve",
            "https://shelf.s3-website-us-east-1.amazonaws.com/index.html",
            false,
        ),
        (
            "the dotted spelling of a website endpoint over plaintext",
            "http://shelf.s3-website.us-east-1.amazonaws.com/index.html",
            true,
        ),
        (
            "the dotted spelling of a website endpoint over TLS",
            "https://shelf.s3-website.us-east-1.amazonaws.com/index.html",
            false,
        ),
        (
            "a host whose label merely begins like the website endpoint, over plaintext",
            "http://shelf.s3-websitely.us-east-1.amazonaws.com/index.html",
            false,
        ),
        (
            "a per-resource listener over plaintext",
            "http://127.0.0.1:38211/index.html",
            true,
        ),
        (
            "a per-resource listener over TLS",
            "https://127.0.0.1:38211/index.html",
            true,
        ),
        (
            "a host nothing here serves, over plaintext",
            "http://origin.example.com/index.html",
            true,
        ),
        (
            "a distribution's own edge domain, over plaintext",
            "http://d111111abcdef8.cloudfront.net/index.html",
            true,
        ),
    ];
    for (case, uri, expected) in cases {
        let actual = origin_accepts_scheme(uri);
        assert_eq!(
            actual, expected,
            "case {case:?} ({uri:?}): expected origin_accepts_scheme == {expected}, got \
                 {actual}"
        );
    }
    // An origin-form URI names no scheme at all, so there is nothing to refuse.
    assert!(
        origin_accepts_scheme("/index.html"),
        "an origin-form URI names no scheme, so it is addressed over the one the request \
             is made on"
    );
}

#[test]
fn an_origin_request_carries_no_viewer_credential_whatever_the_policy_forwards() {
    // The rows differ along the origin request policy's header behaviour — none (the
    // default), `allViewer` and `allViewerExceptHostHeader`, the AWS-managed policies
    // whose ids `MANAGED_ORIGIN_REQUEST_POLICIES` publishes — along the viewer header
    // asked about (the two the credential travels in, a hop-by-hop header, an ordinary
    // one) and along the request's METHOD, because the credential this emulator relays
    // to no origin is relayed to none on any of them.
    const ALL_VIEWER: &str = "216adef6-5c7f-47e4-b989-5492eafa07d3";
    const ALL_VIEWER_EXCEPT_HOST: &str = "b689b0a8-53d0-40ab-baf2-68738e2966ac";
    let store = ComAmazonawsCloudfrontCloudfront20200531State::default();
    let origin = Origin {
        id: "primary".to_string(),
        domain_name: "origin.example.com".to_string(),
        ..Origin::default()
    };
    // (case, the policy the behaviour names, the viewer header, the method, forwarded)
    type Row = (&'static str, Option<&'static str>, &'static str, &'static str, bool);
    let cases: [Row; 10] = [
        ("no policy, a credential on a GET", None, "authorization", "GET", false),
        ("no policy, a credential on a HEAD", None, "authorization", "HEAD", false),
        (
            "no policy, a session token on a GET",
            None,
            "x-amz-security-token",
            "GET",
            false,
        ),
        ("no policy, an ordinary header", None, "accept-language", "GET", false),
        (
            "no policy, a credential on a POST",
            None,
            "authorization",
            "POST",
            false,
        ),
        (
            "allViewer, a credential the behaviour names",
            Some(ALL_VIEWER),
            "authorization",
            "GET",
            false,
        ),
        (
            "allViewer, a session token the behaviour names",
            Some(ALL_VIEWER),
            "x-amz-security-token",
            "PUT",
            false,
        ),
        (
            "allViewer, a hop-by-hop header",
            Some(ALL_VIEWER),
            "connection",
            "GET",
            false,
        ),
        (
            "allViewerExceptHostHeader, an ordinary header the behaviour names",
            Some(ALL_VIEWER_EXCEPT_HOST),
            "accept-language",
            "GET",
            true,
        ),
        (
            "a policy this account does not hold",
            Some("00000000-0000-0000-0000-000000000000"),
            "accept-language",
            "GET",
            false,
        ),
    ];
    for (case, policy, header, method, forwarded) in cases {
        let behavior = Behavior {
            path_pattern: None,
            realtime_log_config_arn: None,
            target_origin_id: "primary".to_string(),
            viewer_protocol_policy: "allow-all".to_string(),
            allowed_methods: None,
            trusted_key_groups: None,
            cache_policy_id: None,
            origin_request_policy_id: policy.map(str::to_string),
            compress: false,
            min_ttl: None,
            default_ttl: None,
            max_ttl: None,
            forwarded_values: None,
            response_headers_policy_id: None,
            field_level_encryption_id: None,
            function_associations: None,
            lambda_function_associations: None,
        };
        let built = ::http::Request::builder()
            .method(method)
            .uri("/index.html")
            .body(SdkBody::empty())
            .expect("a well-formed request");
        let mut req = RtRequest::try_from(built).expect("a runtime-API request");
        req.headers_mut().insert(header.to_string(), "v".to_string());
        let headers =
            origin_request_headers(&store, &behavior, &origin, &req, "cfid", VIEWER_HOST);
        let carried = headers.iter().any(|(name, _)| name == header);
        assert_eq!(
            carried, forwarded,
            "case {case:?}: expected the origin request to carry {header:?} == \
                 {forwarded}, got {carried} (headers {headers:?})"
        );
        // The origin here carries no `CustomOriginConfig`, so it is an Amazon S3 origin:
        // such an origin is addressed by its OWN host whatever a behaviour forwards
        // ([`origin_request_host`]), and the request names what it is being made for.
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "host" && value == &origin.domain_name)
                && headers.iter().any(|(name, _)| name == X_AMZ_CF_ID),
            "case {case:?}: expected the origin's host and an X-Amz-Cf-Id, got \
                 {headers:?}"
        );
    }
}

/// The host the viewer addressed in the rows below: the name a distribution publishes.
const VIEWER_HOST: &str = "d111111abcdef8.cloudfront.net";

#[test]
fn an_origin_request_carries_the_viewers_host_exactly_where_the_behaviour_forwards_it() {
    // Rows differ along what the rule generalises over: WHICH reading names the headers
    // that reach the origin (an AWS-managed origin request policy, the legacy
    // `ForwardedValues` whitelist, neither), the header BEHAVIOUR that reading carries
    // (`allViewer`, `allViewerExceptHostHeader`, a whitelist that names `Host`, one that
    // names another header, the list naming every header), and the KIND of origin (a
    // custom origin, an Amazon S3 one).
    const ALL_VIEWER: &str = "216adef6-5c7f-47e4-b989-5492eafa07d3";
    const ALL_VIEWER_EXCEPT_HOST: &str = "b689b0a8-53d0-40ab-baf2-68738e2966ac";
    const ORIGIN_HOST: &str = "origin.example.com";
    let store = ComAmazonawsCloudfrontCloudfront20200531State::default();
    let custom = Some(CustomOriginConfig {
        http_port: 80,
        https_port: 443,
        origin_protocol_policy: "https-only".to_string(),
        ..CustomOriginConfig::default()
    });
    let whitelist = |names: &[&str]| ForwardedValues {
        query_string: false,
        headers: Some(crate::ops::types::Headers {
            quantity: i32::try_from(names.len()).expect("a small list"),
            items: Some(names.iter().map(|name| (*name).to_string()).collect()),
        }),
        ..ForwardedValues::default()
    };
    // (case, the origin request policy, the legacy member, a custom origin, the host)
    type Row = (&'static str, Option<&'static str>, Option<ForwardedValues>, bool, &'static str);
    let cases: [Row; 8] = [
        ("no policy and no legacy member", None, None, true, ORIGIN_HOST),
        ("allViewer over a custom origin", Some(ALL_VIEWER), None, true, VIEWER_HOST),
        ("allViewer over an S3 origin", Some(ALL_VIEWER), None, false, ORIGIN_HOST),
        (
            "allViewerExceptHostHeader over a custom origin",
            Some(ALL_VIEWER_EXCEPT_HOST),
            None,
            true,
            ORIGIN_HOST,
        ),
        (
            "a legacy whitelist naming Host",
            None,
            Some(whitelist(&["Host"])),
            true,
            VIEWER_HOST,
        ),
        (
            "a legacy whitelist naming another header",
            None,
            Some(whitelist(&["Accept-Language"])),
            true,
            ORIGIN_HOST,
        ),
        (
            "a legacy whitelist naming every header",
            None,
            Some(whitelist(&["*"])),
            true,
            VIEWER_HOST,
        ),
        (
            "a legacy whitelist naming every header, over an S3 origin",
            None,
            Some(whitelist(&["*"])),
            false,
            ORIGIN_HOST,
        ),
    ];
    for (case, policy, forwarded_values, custom_origin, expected) in cases {
        let origin = Origin {
            id: "primary".to_string(),
            domain_name: ORIGIN_HOST.to_string(),
            custom_origin_config: custom_origin.then(|| custom.clone().expect("the config")),
            ..Origin::default()
        };
        let behavior = Behavior {
            path_pattern: None,
            realtime_log_config_arn: None,
            target_origin_id: "primary".to_string(),
            viewer_protocol_policy: "allow-all".to_string(),
            allowed_methods: None,
            trusted_key_groups: None,
            cache_policy_id: None,
            origin_request_policy_id: policy.map(str::to_string),
            compress: false,
            min_ttl: None,
            default_ttl: None,
            max_ttl: None,
            forwarded_values,
            response_headers_policy_id: None,
            field_level_encryption_id: None,
            function_associations: None,
            lambda_function_associations: None,
        };
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri("/index.html").expect("a valid URI");
        req.headers_mut()
            .insert("host".to_string(), VIEWER_HOST.to_string());
        let headers =
            origin_request_headers(&store, &behavior, &origin, &req, "cfid", VIEWER_HOST);
        let carried: Vec<&String> = headers
            .iter()
            .filter(|(name, _)| name == "host")
            .map(|(_, value)| value)
            .collect();
        assert_eq!(
            carried,
            vec![&expected.to_string()],
            "case {case:?}: expected the origin request to carry the one host \
                 {expected:?}, got {carried:?} (headers {headers:?})"
        );
    }
}

#[test]
fn an_alias_may_not_claim_an_address_this_emulator_is_reached_at() {
    // Rows differ along the shape of the stored alternate domain name: a customer
    // domain, the loopback names and IP literals a client on this machine dials, a
    // host that names a service endpoint, and the empty name.
    let cases: [(&str, &str, bool); 11] = [
        ("a customer domain", "cdn.example.com", true),
        ("a wildcard-free subdomain", "assets.cdn.example.com", true),
        ("the loopback name", "localhost", false),
        ("the loopback name with a port", "localhost:9297", false),
        ("a name under the loopback name", "edge.localhost", false),
        ("an IPv4 literal", "127.0.0.1", false),
        ("an IPv4 literal with a port", "127.0.0.1:9297", false),
        ("a bracketed IPv6 literal", "[::1]:9297", false),
        ("another service's endpoint", "s3.us-east-1.amazonaws.com", false),
        ("an edge domain CloudFront assigns", "d111.cloudfront.net", false),
        ("the empty name", "", false),
    ];
    for (case, alias, expected) in cases {
        let actual = addressable_alias(alias);
        assert_eq!(
            actual, expected,
            "case {case:?} ({alias:?}): expected addressable_alias == {expected}, got \
                 {actual}"
        );
    }
}

/// The policies one behaviour names, held in a store so the shaping below reads them
/// exactly as a served request does: a cache policy that keys on one cookie and
/// normalizes the content coding, and an origin request policy that forwards one other
/// cookie, one viewer header and one header the EDGE generates.
fn store_with_policies() -> ComAmazonawsCloudfrontCloudfront20200531State {
    use crate::ops::types::{
        CachePolicyConfig, CachePolicyCookiesConfig, CachePolicyHeadersConfig,
        CachePolicyQueryStringsConfig, CookieNames, Headers,
        OriginRequestPolicyConfig, OriginRequestPolicyCookiesConfig,
        OriginRequestPolicyHeadersConfig, OriginRequestPolicyQueryStringsConfig,
        ParametersInCacheKeyAndForwardedToOrigin, QueryStringNames,
    };
    let name_list = |names: &[&str]| {
        Some((
            i32::try_from(names.len()).unwrap_or_default(),
            Some(names.iter().map(|name| (*name).to_string()).collect()),
        ))
    };
    let mut store = ComAmazonawsCloudfrontCloudfront20200531State::default();
    let cookies = name_list(&["sim_key"]).expect("the list is not empty");
    store.cache_policies.insert(
        CACHE_POLICY.to_string(),
        crate::state::StoredCachePolicy {
            id: CACHE_POLICY.to_string(),
            config: CachePolicyConfig {
                name: "keys-on-one-cookie".to_string(),
                min_ttl: 0,
                default_ttl: Some(60),
                max_ttl: Some(60),
                comment: None,
                parameters_in_cache_key_and_forwarded_to_origin: Some(
                    ParametersInCacheKeyAndForwardedToOrigin {
                        enable_accept_encoding_gzip: true,
                        enable_accept_encoding_brotli: Some(true),
                        headers_config: CachePolicyHeadersConfig {
                            header_behavior: "none".to_string(),
                            headers: None,
                        },
                        cookies_config: CachePolicyCookiesConfig {
                            cookie_behavior: "whitelist".to_string(),
                            cookies: Some(CookieNames {
                                quantity: cookies.0,
                                items: cookies.1,
                            }),
                        },
                        query_strings_config: CachePolicyQueryStringsConfig {
                            query_string_behavior: "none".to_string(),
                            query_strings: None,
                        },
                    },
                ),
            },
            last_modified_time: SystemTime::UNIX_EPOCH,
            etag: "E1".to_string(),
        },
    );
    let headers = name_list(&["x-fwd", "CloudFront-Viewer-Country"])
        .expect("the list is not empty");
    let forwarded_cookies = name_list(&["sim_fwd"]).expect("the list is not empty");
    let forwarded_queries = name_list(&["keep"]).expect("the list is not empty");
    store.origin_request_policies.insert(
        ORIGIN_REQUEST_POLICY.to_string(),
        crate::state::StoredOriginRequestPolicy {
            id: ORIGIN_REQUEST_POLICY.to_string(),
            config: OriginRequestPolicyConfig {
                name: "forwards-what-the-key-excludes".to_string(),
                comment: None,
                headers_config: OriginRequestPolicyHeadersConfig {
                    header_behavior: "whitelist".to_string(),
                    headers: Some(Headers {
                        quantity: headers.0,
                        items: headers.1,
                    }),
                },
                cookies_config: OriginRequestPolicyCookiesConfig {
                    cookie_behavior: "whitelist".to_string(),
                    cookies: Some(CookieNames {
                        quantity: forwarded_cookies.0,
                        items: forwarded_cookies.1,
                    }),
                },
                query_strings_config: OriginRequestPolicyQueryStringsConfig {
                    query_string_behavior: "whitelist".to_string(),
                    query_strings: Some(QueryStringNames {
                        quantity: forwarded_queries.0,
                        items: forwarded_queries.1,
                    }),
                },
            },
            last_modified_time: SystemTime::UNIX_EPOCH,
            etag: "E2".to_string(),
        },
    );
    store
}

/// The id the cache policy of [`store_with_policies`] is held under.
const CACHE_POLICY: &str = "11111111-1111-1111-1111-111111111111";

/// The id the origin request policy of [`store_with_policies`] is held under.
const ORIGIN_REQUEST_POLICY: &str = "22222222-2222-2222-2222-222222222222";

/// RULE: the origin request carries the UNION of what the cache key names and what the
/// origin request policy forwards, and nothing else — and a header CloudFront generates
/// itself carries the EDGE's value, never a value the viewer stated for it.
///
/// The rows differ along what the viewer sent and what is asked about: a cookie the cache
/// key names, a cookie only the origin request policy names, a cookie neither names, a
/// header the policy names, a header it does not, a header the edge generates (both for a
/// viewer that stated nothing and for one that stated a value of its own), and the
/// content coding the cache policy normalizes.
#[test]
fn the_origin_request_carries_what_the_policies_name_and_the_edge_generated_values() {
    let store = store_with_policies();
    let origin = Origin {
        id: "primary".to_string(),
        domain_name: "origin.example.com".to_string(),
        ..Origin::default()
    };
    let behavior = behavior_under_test();
    // What one viewer sent: two cookies the policies name and one neither does, a header
    // the policy names and one it does not, a value spoofed for a header the edge
    // generates, and a content coding it accepts.
    let mut req = RtRequest::new(SdkBody::empty());
    req.set_uri("/obj?keep=1&drop=2").expect("a valid URI");
    for (name, value) in [
        (COOKIE, "sim_fwd=c1; sim_unl=leak; sim_key=k1"),
        ("x-fwd", "forwarded"),
        ("x-unlisted", "withheld"),
        (generated_headers::VIEWER_COUNTRY, "ZZ"),
        (ACCEPT_ENCODING, "gzip, deflate, br"),
    ] {
        req.headers_mut().insert(name.to_string(), value.to_string());
    }
    let headers =
        origin_request_headers(&store, &behavior, &origin, &req, "cfid", VIEWER_HOST);
    let carried = |name: &str| {
        headers
            .iter()
            .find(|(carried, _)| carried == name)
            .map(|(_, value)| value.clone())
    };

    // (case, the header the origin request carries, the value it must carry)
    let cases: [(&str, &str, Option<&str>); 7] = [
        (
            "the cookies the two policies name, in name order, and no other",
            COOKIE,
            Some("sim_fwd=c1; sim_key=k1"),
        ),
        (
            "a viewer header the origin request policy names",
            "x-fwd",
            Some("forwarded"),
        ),
        (
            "a viewer header neither policy names",
            "x-unlisted",
            None,
        ),
        (
            "the country the EDGE resolved, not the one the viewer stated",
            generated_headers::VIEWER_COUNTRY,
            Some("US"),
        ),
        (
            "a generated header the policy does not name",
            generated_headers::FORWARDED_PROTO,
            None,
        ),
        (
            "the normalized content coding, which takes precedence for brotli",
            ACCEPT_ENCODING,
            Some("br"),
        ),
        (
            "the origin's own host, the policies naming no viewer Host beside it",
            "host",
            Some("origin.example.com"),
        ),
    ];
    for (case, header, expected) in cases {
        let actual = carried(header);
        assert_eq!(
            actual.as_deref(),
            expected,
            "case {case:?}: expected the origin request to carry {header:?} == \
                 {expected:?}, got {actual:?} (headers {headers:?})"
        );
    }
}

#[test]
fn the_origin_is_asked_for_the_query_strings_the_policies_name() {
    let store = store_with_policies();
    let keyed = forwarding::cache_key_parameters(&store, &behavior_under_test());
    let forwarded = forwarding::forwarded_parameters(&store, &behavior_under_test());
    // Rows differ along the viewer's query: a named parameter, an unnamed one, both, the
    // order they arrived in, and no query at all.
    let cases: [(&str, &str, &str); 5] = [
        ("a named parameter alone", "keep=1", "keep=1"),
        ("an unnamed parameter is withheld", "drop=2", ""),
        (
            "the named one travels and the unnamed one does not",
            "drop=2&keep=1",
            "keep=1",
        ),
        (
            "the viewer's order is kept",
            "keep=1&drop=2&keep=3",
            "keep=1&keep=3",
        ),
        ("no query at all", "", ""),
    ];
    for (case, query, expected) in cases {
        let actual = origin_query(&keyed, &forwarded, query);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the origin to be asked for {expected:?}, got \
                 {actual:?}"
        );
    }
}

/// The behaviour the two shaping tests above drive: it names both policies of
/// [`store_with_policies`] and asks for automatic compression.
fn behavior_under_test() -> Behavior {
    Behavior {
        path_pattern: None,
        realtime_log_config_arn: None,
        target_origin_id: "primary".to_string(),
        viewer_protocol_policy: "allow-all".to_string(),
        allowed_methods: None,
        trusted_key_groups: None,
        cache_policy_id: Some(CACHE_POLICY.to_string()),
        origin_request_policy_id: Some(ORIGIN_REQUEST_POLICY.to_string()),
        compress: true,
        min_ttl: None,
        default_ttl: None,
        max_ttl: None,
        forwarded_values: None,
        response_headers_policy_id: None,
        field_level_encryption_id: None,
        function_associations: None,
        lambda_function_associations: None,
    }
}

#[test]
fn a_request_that_has_already_been_through_an_edge_is_recognised() {
    // Rows differ along the `Via` the request carries: one an edge of this emulator
    // stamped, one another proxy did, a chain ending in one, and none at all.
    let cases: [(&str, Option<&str>, bool); 4] = [
        ("no Via at all", None, false),
        (
            "a Via an edge stamped",
            Some("1.1 cfid.cloudfront.net (CloudFront)"),
            true,
        ),
        ("another proxy's Via", Some("1.1 proxy.internal"), false),
        (
            "a chain whose last hop is an edge",
            Some("1.1 proxy.internal, 1.1 cfid.cloudfront.net (CloudFront)"),
            true,
        ),
    ];
    for (case, via, expected) in cases {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri("/index.html").expect("a valid URI");
        if let Some(via) = via {
            req.headers_mut().insert("via".to_string(), via.to_string());
        }
        let actual = carries_cloudfront_via(&req);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected carries_cloudfront_via == {expected}, got {actual}"
        );
    }
}
