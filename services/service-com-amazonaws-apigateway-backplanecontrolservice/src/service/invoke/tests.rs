// SPDX-License-Identifier: CC0-1.0

//! The pure rules this module owns that a wire test cannot vary cheaply: which
//! host is an `execute-api` endpoint, how specifically a deployed resource path
//! matches a request path, and how an integration's `uri` is addressed. The served
//! behaviour is pinned end to end in `tests/all/execute_api_data_plane.rs`.

use super::*;

#[test]
fn an_execute_api_host_is_recognised_by_its_second_label() {
    // Rows differ along the api id, the region, the port, the scheme-level userinfo
    // and the endpoint variant — everything the rule generalises over — plus the hosts
    // that are NOT an api's invoke endpoint.
    let cases: &[(&str, Option<&str>)] = &[
        ("abc1230000.execute-api.us-east-1.amazonaws.com", Some("abc1230000")),
        ("0123456789.execute-api.eu-west-3.amazonaws.com", Some("0123456789")),
        ("a.execute-api.ap-southeast-2.amazonaws.com:443", Some("a")),
        (
            "abc1230000.execute-api-fips.us-gov-west-1.amazonaws.com",
            Some("abc1230000"),
        ),
        ("abc1230000.execute-api.cn-north-1.amazonaws.com.cn", Some("abc1230000")),
        // A label that merely starts with the endpoint's: a different host.
        ("abc1230000.execute-apifoo.us-east-1.amazonaws.com", None),
        // The control plane, whose first label names the service.
        ("apigateway.us-east-1.amazonaws.com", None),
        // The management API's own endpoint host: no api id in front of the label.
        ("execute-api.us-east-1.amazonaws.com", None),
        // Not an endpoint at all.
        ("127.0.0.1:9297", None),
        ("localhost", None),
        ("abc1230000.execute-api", None),
    ];
    for (host, expected) in cases {
        let resolved = api_id_of_host(host);
        assert_eq!(
            resolved, *expected,
            "case {host:?}: expected the api id {expected:?}, got {resolved:?}"
        );
    }
}

#[test]
fn a_vpc_endpoints_own_dns_name_names_the_endpoint_and_not_an_api() {
    // Rows differ along the id width EC2's pattern admits, the zonal suffix an
    // endpoint's per-zone name carries, the region, the port and the endpoint variant —
    // plus the hosts that name NO endpoint, which keep resolving by api id.
    let cases: &[(&str, Option<&str>)] = &[
        (
            "vpce-0123456789abcdef0.execute-api.us-west-2.vpce.amazonaws.com",
            Some("vpce-0123456789abcdef0"),
        ),
        // The zonal name of the SAME endpoint, which EC2 reports beside the regional
        // one: the zone is a suffix of the label, not part of the id.
        (
            "vpce-0123456789abcdef0-us-west-2a.execute-api.us-west-2.vpce.amazonaws.com",
            Some("vpce-0123456789abcdef0"),
        ),
        // AWS's own endpoint-specific public name carries a suffix of its own.
        (
            "vpce-0123456789abcdef0-01234567.execute-api.eu-west-1.vpce.amazonaws.com:443",
            Some("vpce-0123456789abcdef0"),
        ),
        // The eight-digit width the id pattern also admits.
        (
            "vpce-0123abcd.execute-api-fips.us-gov-west-1.vpce.amazonaws.com",
            Some("vpce-0123abcd"),
        ),
        // An api id that merely begins with the prefix is no endpoint id: neither
        // width, and not hexadecimal.
        ("vpce-xyz.execute-api.us-west-2.amazonaws.com", None),
        ("vpcezzzzzzz.execute-api.us-west-2.amazonaws.com", None),
        ("abc1230000.execute-api.us-east-1.amazonaws.com", None),
        // Not an `execute-api` endpoint host at all.
        ("vpce-0123456789abcdef0.s3.us-west-2.vpce.amazonaws.com", None),
        ("localhost", None),
    ];
    for (host, expected) in cases {
        let resolved = endpoint_of_host(host);
        assert_eq!(
            resolved, *expected,
            "case {host:?}: expected the endpoint id {expected:?}, got {resolved:?}"
        );
    }
}

#[test]
fn the_data_plane_path_prefix_names_the_api_before_the_stage() {
    // Rows differ along what follows the prefix (an api id with an invoke path, with a
    // stage alone, with nothing), along the spelling of the prefix itself (extra
    // slashes, a label that merely starts with it, one segment of it alone) and along
    // paths that carry no prefix at all, which the other addressings read.
    let cases: &[(&str, Option<(&str, &str)>)] = &[
        (
            "/_aws/execute-api/abc1230000/prod/pets",
            Some(("abc1230000", "prod/pets")),
        ),
        (
            "/_aws/execute-api/abc1230000/prod",
            Some(("abc1230000", "prod")),
        ),
        ("/_aws/execute-api/abc1230000", Some(("abc1230000", ""))),
        // Empty segments are not segments, so the same address spelled with extra
        // slashes names the same api, and the invoke path it leaves is read by
        // [`split_stage`], which drops them too.
        (
            "//_aws//execute-api//abc1230000//prod//pets",
            Some(("abc1230000", "/prod//pets")),
        ),
        // The prefix is read by exact segment equality, so a segment that merely starts
        // with it is a different path.
        ("/_awsx/execute-api/abc1230000/prod", None),
        ("/_aws/execute-apifoo/abc1230000/prod", None),
        // The prefix with nothing after it names no api.
        ("/_aws/execute-api", None),
        ("/_aws", None),
        // No prefix at all: the api id or the stage leads, for the other addressings.
        ("/abc1230000/prod/pets", None),
        ("/prod/pets", None),
        ("/", None),
        ("", None),
    ];
    for (path, expected) in cases {
        let split = split_execute_api_prefix(path);
        assert_eq!(
            split, *expected,
            "case {path:?}: expected the api id and invoke path {expected:?}, got {split:?}"
        );
    }
}

#[test]
fn a_template_carries_a_directive_only_where_it_states_one() {
    // Rows differ along the construct and along the literal characters that merely
    // LOOK like one: a template that states no directive is RENDERED, so the detector
    // must not claim it has to be executed — and a reference is not a directive.
    let cases: &[(&str, bool)] = &[
        (r#"{"ok":true}"#, false),
        ("", false),
        // Literal `#` and `$` that introduce nothing.
        (r##"{"color":"#fff"}"##, false),
        (r#"{"price":"$5"}"#, false),
        (r##"{"anchor":"#"}"##, false),
        // Non-ASCII text, which nothing restricts a stored template to: the scan must
        // answer for it rather than fail on a byte index inside a character.
        (r#"{"msg":"café"}"#, false),
        ("☃", false),
        ("🙂 $ 🙂", false),
        ("☃#set($x = 1)", true),
        // References are the half that is rendered, not executed.
        (r#"{"msg":"café $input.params('x')"}"#, false),
        (r#"{"id":"$input.params('id')"}"#, false),
        (r#"{"id":"${input}"}"#, false),
        (r#"{"id":"$!input"}"#, false),
        // Directives and comments.
        ("#set($root = $input.path('$'))", true),
        ("#if($x)a#end", true),
        // A word that merely starts with a directive's name is literal text.
        ("#sets", false),
        ("#ifconfig", false),
        (r##"{"tag":"#end2end"}"##, false),
        ("#end", true),
        ("#set($x = 1)", true),
        ("## a comment", true),
        ("#* a block comment *#", true),
    ];
    for (template, directive) in cases {
        let detected = carries_velocity_directive(template);
        assert_eq!(
            detected, *directive,
            "case {template:?}: expected carries_velocity_directive to be {directive}, \
                 got {detected}"
        );
    }
}

#[test]
fn a_literal_segment_beats_a_parameter_which_beats_a_greedy_parameter() {
    // Rows differ along the template's form and the request's segment count: the
    // dimensions the specificity rule generalises over.
    let cases: &[(&str, &str, bool)] = &[
        ("/", "/", true),
        ("/pets", "/pets", true),
        ("/pets", "/pets/1", false),
        ("/pets/{petId}", "/pets/1", true),
        ("/pets/{petId}", "/pets", false),
        ("/pets/{petId}", "/pets/1/toys", false),
        ("/{proxy+}", "/pets/1/toys", true),
        ("/{proxy+}", "/", false),
        ("/pets/{proxy+}", "/pets/1/toys", true),
    ];
    for (template, request, matches) in cases {
        let scored = match_score(template, request).is_some();
        assert_eq!(
            scored, *matches,
            "case {template:?} vs {request:?}: expected a match of {matches}, got {scored}"
        );
    }
    let literal = match_score("/pets/mine", "/pets/mine").expect("the literal matches");
    let parameter = match_score("/pets/{petId}", "/pets/mine").expect("the parameter matches");
    let greedy = match_score("/{proxy+}", "/pets/mine").expect("the greedy parameter matches");
    assert!(
        literal > parameter && parameter > greedy,
        "case specificity: expected literal > parameter > greedy, got \
             {literal:?} > {parameter:?} > {greedy:?}"
    );
}

/// RULE: an integration `uri`'s `{name}` placeholders are filled in ONE pass over the `uri`
/// itself, so a value substituted for one name is never scanned for another name's
/// placeholder — the values are the caller's, so rescanning them would make the address the
/// product of their sizes rather than their sum.
#[test]
fn a_substituted_path_parameter_is_never_scanned_for_another_placeholder() {
    let mapped = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    };
    /// One row: the integration's `uri`, the values mapped onto its path parameters, and the
    /// address the rule owes it.
    struct Addressing<'a> {
        uri: &'a str,
        mapped: &'a [(&'a str, &'a str)],
        expected: &'a str,
    }
    let case = |uri, mapped, expected| Addressing {
        uri,
        mapped,
        expected,
    };
    // Rows differ along what the mapped values carry and which placeholders the `uri` has:
    // a plain value, a value that spells another name's placeholder (twice, and in the name
    // that sorts FIRST so a per-name rewrite would rescan it), a placeholder no mapping
    // names, repeated placeholders of one name, and braces that are text rather than a
    // placeholder.
    let cases = [
        case("/orders/{id}", &[("id", "42")], "/orders/42"),
        case(
            "/a/{alpha}/b/{beta}",
            &[("alpha", "{beta}{beta}"), ("beta", "B")],
            "/a/{beta}{beta}/b/B",
        ),
        case(
            "/a/{alpha}/b/{beta}",
            &[("beta", "{alpha}"), ("alpha", "A")],
            "/a/A/b/{alpha}",
        ),
        case("/{proxy}", &[], "/{proxy}"),
        case("/{id}/{id}", &[("id", "7")], "/7/7"),
        case("/not-a-placeholder", &[("id", "7")], "/not-a-placeholder"),
        case("/open{id", &[("id", "7")], "/open{id"),
        case("/{}/x", &[("id", "7")], "/{}/x"),
    ];
    for row in &cases {
        let addressed = expanded_uri(row.uri, &mapped(row.mapped));
        assert_eq!(
            addressed, row.expected,
            "case {:?} with {:?}: expected {:?}, got {addressed:?}",
            row.uri, row.mapped, row.expected
        );
    }
}

/// RULE: a gateway response's template is rendered in ONE pass over the TEMPLATE, so a value
/// substituted for one `$context` name is never scanned for another name's reference — the
/// refusal's message and validation detail carry text the request influenced, so rescanning
/// them would make the body the PRODUCT of their sizes rather than their sum — and a
/// rendering that passes the body bound renders nothing at all, so the caller is answered the
/// default body.
#[test]
fn a_substituted_gateway_context_value_is_never_scanned_for_another_reference() {
    let refusal = |message: Cow<'static, str>, detail: Cow<'static, str>| GatewayRefusal {
        response_type: DEFAULT_4XX,
        message,
        error_type: None,
        validation_detail: detail,
        message_member: MESSAGE_MEMBER,
    };
    /// One row: the stored template, the refusal's message and validation detail, and the
    /// body the rule owes — `None` for a rendering the bound refuses.
    struct Rendering {
        case: &'static str,
        template: String,
        message: Cow<'static, str>,
        detail: Cow<'static, str>,
        expected: Option<String>,
    }
    // A value whose own text spells a reference: substituted, it must travel into the body
    // as text. `validationErrorString` is substituted FIRST by name order, so a per-name
    // rewrite of the whole template would expand what it left behind.
    let spells_a_reference = "$context.error.message$context.error.message";
    // The bound row: a value large enough that repeating its reference passes the cap.
    let large = "x".repeat(64 * 1024);
    let cases = vec![
        Rendering {
            case: "API Gateway's own default template",
            template: r#"{"message":$context.error.messageString}"#.to_string(),
            message: Cow::Borrowed("Missing Authentication Token"),
            detail: Cow::Borrowed(""),
            expected: Some(r#"{"message":"Missing Authentication Token"}"#.to_string()),
        },
        Rendering {
            case: "the braced spelling of the same reference",
            template: r#"{"m":"${context.error.message}"}"#.to_string(),
            message: Cow::Borrowed("Forbidden"),
            detail: Cow::Borrowed(""),
            expected: Some(r#"{"m":"Forbidden"}"#.to_string()),
        },
        Rendering {
            case: "a validation detail that itself spells a reference",
            template: r#"{"d":"$context.error.validationErrorString"}"#.to_string(),
            message: Cow::Borrowed("Invalid request body"),
            detail: Cow::Borrowed(spells_a_reference),
            expected: Some(format!(r#"{{"d":"{spells_a_reference}"}}"#)),
        },
        Rendering {
            case: "a message that spells the validation detail's reference, both rendered",
            template: r#"$context.error.validationErrorString|$context.error.message"#
                .to_string(),
            message: Cow::Borrowed("$context.error.validationErrorString"),
            detail: Cow::Borrowed("$context.error.message"),
            expected: Some(
                "$context.error.message|$context.error.validationErrorString".to_string(),
            ),
        },
        Rendering {
            case: "a reference this refusal resolves nothing for, and the response type",
            template: r#"{"id":"$context.requestId","t":"$context.error.responseType"}"#
                .to_string(),
            message: Cow::Borrowed("Bad request"),
            detail: Cow::Borrowed(""),
            expected: Some(format!(
                r#"{{"id":"$context.requestId","t":"{DEFAULT_4XX}"}}"#
            )),
        },
        Rendering {
            case: "a `$` that is text rather than a reference",
            template: "a price of $5.00".to_string(),
            message: Cow::Borrowed("Bad request"),
            detail: Cow::Borrowed(""),
            expected: Some("a price of $5.00".to_string()),
        },
        Rendering {
            case: "a template whose rendering passes the body bound",
            template: "$context.error.message".repeat(8),
            message: Cow::Owned(large.clone()),
            detail: Cow::Borrowed(""),
            expected: None,
        },
    ];
    for row in &cases {
        let rendered = rendered_gateway_template(
            &row.template,
            &refusal(row.message.clone(), row.detail.clone()),
        );
        assert_eq!(
            rendered, row.expected,
            "case {:?}: expected {:?}, got {:?}",
            row.case,
            row.expected.as_deref().map(|body| body.len()),
            rendered.as_deref().map(str::len)
        );
    }
}
