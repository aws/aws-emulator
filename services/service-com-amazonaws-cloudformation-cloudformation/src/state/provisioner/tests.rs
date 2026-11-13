// SPDX-License-Identifier: CC0-1.0

//! The unit tests of [`super`]'s per-type provisioning arms: what each arm renders for one
//! declaration, held to the member names and shapes the owning service's API takes.

use super::*;

/// One declared `AWS::Route53::RecordSet` as the record-set renderer takes it.
fn declared_record_set(properties: Value) -> StoredResource {
    StoredResource {
        logical_id: "Record".to_string(),
        resource_type: ROUTE53_RECORD_SET.to_string(),
        physical_resource_id: "a.example.com".to_string(),
        attributes: BTreeMap::new(),
        metadata: None,
        properties,
        materialised: false,
        deletion_policy: None,
        update_replace_policy: None,
    }
}

/// One row of the record-set rendering table: the case, the properties a template
/// declares, and the element they render as — `None` where the declaration is refused.
type RecordSetRow = (&'static str, fn() -> Value, Option<&'static str>);

/// The `ResourceRecordSet` a change batch carries is the declaration's own members, with
/// the record VALUES wrapped as the `ResourceRecord` structures the wire takes and every
/// nested routing structure rendered member by member — and a property or nested member
/// the catalogue does not name is REFUSED rather than dropped.
///
/// The rows span the shapes [`RECORD_SET_MEMBERS`] distinguishes: scalars, one and
/// several record values, each nested structure the type publishes, a value carrying XML
/// markup, a property of a different resource type, and a member no structure has.
#[test]
fn a_record_set_renders_the_members_its_declaration_states() {
    // (case, the declared properties, the element it renders or None where it is refused)
    let rows: &[RecordSetRow] = &[
        (
            "a simple record with one value",
            || {
                serde_json::json!({
                    "Name": "a.example.com",
                    "Type": "A",
                    "TTL": "300",
                    "ResourceRecords": ["192.0.2.10"],
                    "HostedZoneId": "Z0123456789ABCD"
                })
            },
            Some(
                "<ResourceRecordSet><Name>a.example.com</Name><Type>A</Type>\
                     <TTL>300</TTL><ResourceRecords><ResourceRecord><Value>192.0.2.10</Value>\
                     </ResourceRecord></ResourceRecords></ResourceRecordSet>",
            ),
        ),
        (
            "a weighted record with several values and a set identifier",
            || {
                serde_json::json!({
                    "Name": "mx.example.com",
                    "Type": "MX",
                    "SetIdentifier": "primary",
                    "Weight": 70,
                    "TTL": 60,
                    "ResourceRecords": ["10 one.example.com.", "20 two.example.com."],
                    "HostedZoneName": "example.com."
                })
            },
            Some(
                "<ResourceRecordSet><Name>mx.example.com</Name><Type>MX</Type>\
                     <SetIdentifier>primary</SetIdentifier><Weight>70</Weight><TTL>60</TTL>\
                     <ResourceRecords><ResourceRecord><Value>10 one.example.com.</Value>\
                     </ResourceRecord><ResourceRecord><Value>20 two.example.com.</Value>\
                     </ResourceRecord></ResourceRecords></ResourceRecordSet>",
            ),
        ),
        (
            "an alias record, whose target is a nested structure and which has no TTL",
            || {
                serde_json::json!({
                    "Name": "alias.example.com",
                    "Type": "A",
                    "AliasTarget": {
                        "HostedZoneId": "Z2FDTNDATAQYW2",
                        "DNSName": "d111111abcdef8.cloudfront.net.",
                        "EvaluateTargetHealth": false
                    }
                })
            },
            Some(
                "<ResourceRecordSet><Name>alias.example.com</Name><Type>A</Type>\
                     <AliasTarget><HostedZoneId>Z2FDTNDATAQYW2</HostedZoneId>\
                     <DNSName>d111111abcdef8.cloudfront.net.</DNSName>\
                     <EvaluateTargetHealth>false</EvaluateTargetHealth></AliasTarget>\
                     </ResourceRecordSet>",
            ),
        ),
        (
            "a geolocation record and a health check",
            || {
                serde_json::json!({
                    "Name": "geo.example.com",
                    "Type": "A",
                    "SetIdentifier": "africa",
                    "GeoLocation": {"ContinentCode": "AF"},
                    "TTL": "60",
                    "ResourceRecords": ["192.0.2.11"],
                    "HealthCheckId": "a-health-check"
                })
            },
            Some(
                "<ResourceRecordSet><Name>geo.example.com</Name><Type>A</Type>\
                     <SetIdentifier>africa</SetIdentifier>\
                     <GeoLocation><ContinentCode>AF</ContinentCode></GeoLocation>\
                     <TTL>60</TTL><ResourceRecords><ResourceRecord><Value>192.0.2.11</Value>\
                     </ResourceRecord></ResourceRecords>\
                     <HealthCheckId>a-health-check</HealthCheckId></ResourceRecordSet>",
            ),
        ),
        (
            "a geoproximity record, whose coordinates nest one structure inside another",
            || {
                serde_json::json!({
                    "Name": "near.example.com",
                    "Type": "A",
                    "SetIdentifier": "coast",
                    "TTL": "60",
                    "ResourceRecords": ["192.0.2.12"],
                    "GeoProximityLocation": {
                        "Coordinates": {"Latitude": "47.6", "Longitude": "-122.3"},
                        "Bias": "10"
                    }
                })
            },
            Some(
                "<ResourceRecordSet><Name>near.example.com</Name><Type>A</Type>\
                     <SetIdentifier>coast</SetIdentifier><TTL>60</TTL>\
                     <ResourceRecords><ResourceRecord><Value>192.0.2.12</Value></ResourceRecord>\
                     </ResourceRecords><GeoProximityLocation>\
                     <Coordinates><Latitude>47.6</Latitude><Longitude>-122.3</Longitude>\
                     </Coordinates><Bias>10</Bias></GeoProximityLocation></ResourceRecordSet>",
            ),
        ),
        (
            "a CIDR-routed record, and a failover member beside a latency region",
            || {
                serde_json::json!({
                    "Name": "cidr.example.com",
                    "Type": "A",
                    "SetIdentifier": "block",
                    "Region": "us-west-2",
                    "Failover": "PRIMARY",
                    "MultiValueAnswer": true,
                    "TTL": "60",
                    "ResourceRecords": ["192.0.2.13"],
                    "CidrRoutingConfig": {
                        "CollectionId": "a-collection",
                        "LocationName": "a-location"
                    }
                })
            },
            Some(
                "<ResourceRecordSet><Name>cidr.example.com</Name><Type>A</Type>\
                     <SetIdentifier>block</SetIdentifier><Region>us-west-2</Region>\
                     <Failover>PRIMARY</Failover><MultiValueAnswer>true</MultiValueAnswer>\
                     <TTL>60</TTL><ResourceRecords><ResourceRecord><Value>192.0.2.13</Value>\
                     </ResourceRecord></ResourceRecords>\
                     <CidrRoutingConfig><CollectionId>a-collection</CollectionId>\
                     <LocationName>a-location</LocationName></CidrRoutingConfig>\
                     </ResourceRecordSet>",
            ),
        ),
        (
            "a TXT value carrying XML markup, which reaches the wire escaped",
            || {
                serde_json::json!({
                    "Name": "txt.example.com",
                    "Type": "TXT",
                    "TTL": "60",
                    "ResourceRecords": ["\"a & <b>\""]
                })
            },
            Some(
                "<ResourceRecordSet><Name>txt.example.com</Name><Type>TXT</Type>\
                     <TTL>60</TTL><ResourceRecords><ResourceRecord>\
                     <Value>&quot;a &amp; &lt;b&gt;&quot;</Value></ResourceRecord>\
                     </ResourceRecords></ResourceRecordSet>",
            ),
        ),
        (
            "a property this type does not publish",
            || {
                serde_json::json!({
                    "Name": "a.example.com",
                    "Type": "A",
                    "TTL": "60",
                    "ResourceRecords": ["192.0.2.14"],
                    "TrafficPolicyInstanceId": "an-instance"
                })
            },
            None,
        ),
        (
            "a member no nested structure of this type has",
            || {
                serde_json::json!({
                    "Name": "alias.example.com",
                    "Type": "A",
                    "AliasTarget": {"DNSName": "a.example.com.", "Zone": "Z0123"}
                })
            },
            None,
        ),
        (
            "a declaration with no Type at all",
            || serde_json::json!({ "Name": "a.example.com", "TTL": "60" }),
            None,
        ),
        (
            "record values that are not a list",
            || {
                serde_json::json!({
                    "Name": "a.example.com",
                    "Type": "A",
                    "TTL": "60",
                    "ResourceRecords": "192.0.2.15"
                })
            },
            None,
        ),
    ];

    for (case, declared, expected) in rows {
        let resource = declared_record_set(declared());
        let rendered = record_set_element(&resource);
        match expected {
            Some(expected) => {
                let rendered = rendered
                    .as_ref()
                    .map(String::as_str)
                    .unwrap_or_else(|refusal| {
                        panic!("case {case:?}: expected an element, got {refusal:?}")
                    });
                assert_eq!(
                    rendered, *expected,
                    "case {case:?}: expected {expected:?}, got {rendered:?}"
                );
            }
            None => {
                assert!(
                    rendered.is_err(),
                    "case {case:?}: expected a refusal, got {rendered:?}"
                );
            }
        }
    }
}

/// Every property name becomes the API member it is, and only the first letter moves.
#[test]
fn property_names_become_their_api_members() {
    for (property, expected) in [
        ("RoleArn", "roleArn"),
        ("ActionTypeId", "actionTypeId"),
        ("Name", "name"),
        ("RoleARN", "roleARN"),
        ("", ""),
    ] {
        let actual = lower_initial(property);
        assert_eq!(
            actual, expected,
            "case {property:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// An action's `Configuration` keys are the provider's own and are NOT converted,
/// while every member name around them is.
#[test]
fn action_configuration_keys_are_carried_verbatim() {
    let declared = serde_json::json!({
        "Stages": [{
            "Name": "Source",
            "Actions": [{
                "Name": "Fetch",
                "Configuration": {"S3Bucket": "b", "PollForSourceChanges": "false"}
            }]
        }]
    });
    let converted = camel_cased(&declared, 0).expect("a shallow document is carried");
    let action = &converted["stages"][0]["actions"][0];
    assert_eq!(
        action["name"].as_str(),
        Some("Fetch"),
        "expected the member names converted, got {converted}"
    );
    assert_eq!(
        action["configuration"]["S3Bucket"].as_str(),
        Some("b"),
        "expected the provider's own configuration keys verbatim, got {converted}"
    );
    assert!(
        action["configuration"].get("s3Bucket").is_none(),
        "a configuration key must not be converted, got {converted}"
    );
}

/// A property document deeper than [`MAX_PROPERTY_NESTING`] is REFUSED by every bounded
/// walk over one, and one at the bound is carried.
///
/// The nesting of an evaluated property document is the REQUESTER's choice, and a
/// recursive walk over it that is bounded by the input exhausts the native stack —
/// which aborts the process rather than failing the one stack. The rows differ along
/// the shape the nesting is built from (objects, arrays, and the two alternating) and
/// cover the deepest document each walk carries, one level past it, and far past it.
#[test]
fn a_property_document_nested_past_the_bound_is_refused() {
    /// A document `levels` deep, built from `shape`.
    fn nested(shape: &str, levels: usize) -> Value {
        let mut value = Value::String("leaf".to_string());
        for level in 0..levels {
            value = match shape {
                "objects" => serde_json::json!({ "Member": value }),
                "arrays" => Value::Array(vec![value]),
                // Alternating, so neither arm of the walk is the only one exercised.
                _ if level % 2 == 0 => serde_json::json!({ "Member": value }),
                _ => Value::Array(vec![value]),
            };
        }
        value
    }

    /// Every bounded walk, over one document.
    fn walks(document: &Value) -> Vec<(&'static str, Result<Value, Refusal>)> {
        vec![
            ("camel_cased", camel_cased(document, 0)),
            ("lower_camel_members", lower_camel_members(document, 0)),
            (
                "wafv2_spelled",
                wafv2_spelled(document, 0, Wafv2Spelling::Wire),
            ),
        ]
    }

    for shape in ["objects", "arrays", "alternating"] {
        // The deepest document the walk carries: the leaf sits at depth
        // MAX_PROPERTY_NESTING - 1, so the walk never reaches the bound.
        let carried = nested(shape, MAX_PROPERTY_NESTING - 1);
        for (walk, converted) in walks(&carried) {
            assert!(
                converted.is_ok(),
                "case {walk}({shape} at the bound): expected the document to be carried, \
                     got {:?}",
                converted.err()
            );
        }

        for levels in [MAX_PROPERTY_NESTING + 1, MAX_PROPERTY_NESTING * 4] {
            let too_deep = nested(shape, levels);
            for (walk, refused) in walks(&too_deep) {
                let refusal = refused.err().unwrap_or_else(|| {
                    panic!("case {walk}({shape}, {levels} levels): expected a refusal")
                });
                assert_eq!(
                    refusal.code, "ValidationException",
                    "case {walk}({shape}, {levels} levels): expected ValidationException, got \
                         {refusal:?}"
                );
            }
        }
    }
}

/// A byte-match statement's search value survives the round trip between the two
/// spellings a template declares it in and the base64 the blob travels as.
///
/// The rows differ along the spelling the declaration uses and along what the bytes are:
/// text, text carrying the characters base64's own alphabet uses, a single byte, and
/// bytes that are not text at all (declarable only in the encoded spelling). Each row
/// goes through the walk, so the statement is rewritten where it really sits — nested
/// inside a rule's `Statement`.
#[test]
fn a_byte_match_search_value_round_trips_through_both_template_spellings() {
    // The bytes of each row, which the wire carries as their base64.
    let rows: Vec<(&str, Vec<u8>)> = vec![
        ("plain text", b"sim2real-block".to_vec()),
        ("text with base64's own alphabet", b"a+b/c=d".to_vec()),
        ("a single byte", b"/".to_vec()),
        // Not UTF-8, so a template can only declare these in the encoded spelling.
        ("bytes that are not text", vec![0xff, 0xfe, 0x00, 0x41]),
    ];

    for (case, bytes) in rows {
        let encoded = ::aws_smithy_types::base64::encode(&bytes);
        let text = String::from_utf8(bytes.clone()).ok();
        // Both spellings of the same bytes: the text one where the bytes are text, and
        // the encoded one always.
        let mut declarations: Vec<(&str, String)> = Vec::new();
        if let Some(text) = text.clone() {
            declarations.push((WAFV2_SEARCH_STRING, text));
        }
        declarations.push((WAFV2_SEARCH_STRING_BASE64, encoded.clone()));

        for (spelling, declared) in declarations {
            let mut statement = Map::new();
            statement.insert(spelling.to_string(), Value::String(declared));
            statement.insert(
                "PositionalConstraint".to_string(),
                Value::String("EXACTLY".to_string()),
            );
            let properties = serde_json::json!({
                "Rules": [{
                    "Name": "OneRule",
                    "Statement": { "ByteMatchStatement": Value::Object(statement) },
                }],
            });
            let request =
                wafv2_spelled(&properties, 0, Wafv2Spelling::Wire)
                    .unwrap_or_else(|refusal| {
                        panic!("case {case:?}/{spelling}: expected the walk to carry the \
                                    document, got {refusal:?}")
                    });
            let statement = request
                .pointer("/Rules/0/Statement/ByteMatchStatement")
                .cloned()
                .unwrap_or(Value::Null);
            // The API takes ONE member, base64, whichever spelling was declared.
            assert_eq!(
                statement.get(WAFV2_SEARCH_STRING).and_then(Value::as_str),
                Some(encoded.as_str()),
                "case {case:?}/{spelling}: expected the API to be sent the base64 of the \
                     declared bytes, got {statement}"
            );
            assert!(
                statement.get(WAFV2_SEARCH_STRING_BASE64).is_none(),
                "case {case:?}/{spelling}: expected no encoded-spelling member in the \
                     request, got {statement}"
            );
            assert_eq!(
                statement.get("PositionalConstraint").and_then(Value::as_str),
                Some("EXACTLY"),
                "case {case:?}/{spelling}: expected the statement's other members \
                     carried, got {statement}"
            );

            // And the reported statement carries the value in BOTH spellings, so the
            // drift comparison — which compares only a member the declaration carries —
            // finds it in the one that declaration wrote.
            let reported = wafv2_spelled(&request, 0, Wafv2Spelling::Template)
            .unwrap_or_else(|refusal| {
                panic!("case {case:?}/{spelling}: expected the walk to carry the reported \
                            document, got {refusal:?}")
            });
            let statement = reported
                .pointer("/Rules/0/Statement/ByteMatchStatement")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                statement
                    .get(WAFV2_SEARCH_STRING_BASE64)
                    .and_then(Value::as_str),
                Some(encoded.as_str()),
                "case {case:?}/{spelling}: expected the reported statement to carry the \
                     encoded spelling, got {statement}"
            );
            assert_eq!(
                statement.get(WAFV2_SEARCH_STRING).and_then(Value::as_str),
                text.as_deref(),
                "case {case:?}/{spelling}: expected the reported statement to carry the \
                     text spelling only where the bytes are text, got {statement}"
            );
        }
    }
}

/// A policy document reaches IAM as text whichever way the template spells it.
#[test]
fn policy_documents_render_as_text() {
    for declared in [
        serde_json::json!({"Version": "2012-10-17"}),
        serde_json::json!("{\"Version\":\"2012-10-17\"}"),
    ] {
        let actual = document_text(&declared);
        assert_eq!(
            actual.as_deref(),
            Some(r#"{"Version":"2012-10-17"}"#),
            "case {declared:?}: expected the document as text, got {actual:?}"
        );
    }
}
