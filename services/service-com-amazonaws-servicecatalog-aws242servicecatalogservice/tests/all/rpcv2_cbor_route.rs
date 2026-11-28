// SPDX-License-Identifier: CC0-1.0

//! The OTHER wire format this service declares, driven end to end.
//!
//! The model declares `rpcv2Cbor` beside `awsJson1_1`
//! (`smithy_query(query="service_info")`: protocols `["awsJson1_1", "rpcv2Cbor"]`), so a
//! client may call every operation over
//! `POST /service/AWS242ServiceCatalogService/operation/<Operation>` with
//! `smithy-protocol: rpc-v2-cbor` and a CBOR body, exactly as it may call it over
//! `POST /` with an `X-Amz-Target` and a JSON one.
//!
//! RULE under test: **a request is served in the wire format it ARRIVED in, and the format
//! changes only the envelope — the operation it names, the state it writes and reads, and
//! the failure it earns are the same ones the other format answers.** A service that
//! declares several wire formats is selected by its signing name, which is the same for
//! all of them, so each request carries its own format; nothing about the answer may
//! depend on which format the model happens to declare first.
//!
//! The rows differ along what the rule generalises over: the OPERATION called (two
//! creates, two reads and a paged list), the members a request carries (a portfolio with
//! and without a description, with no tag, one tag and several), the DIRECTION a value
//! crosses the two formats (written on one and read on the other, both ways), success
//! against failure, and the LAYER that answers a failure — a handler's modelled error and
//! the boundary refusal a request earns before any handler runs.

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_servicecatalog_aws242servicecatalogservice::ops::types::{
    CreatePortfolioInput, CreatePortfolioOutput, CreateTagOptionInput, CreateTagOptionOutput,
    DescribePortfolioInput, DescribePortfolioOutput, DescribeTagOptionInput,
    DescribeTagOptionOutput, ListPortfoliosInput, ListPortfoliosOutput, Tag,
    CREATE_PORTFOLIO_INPUT_SCHEMA, CREATE_TAG_OPTION_INPUT_SCHEMA, DESCRIBE_PORTFOLIO_INPUT_SCHEMA,
    DESCRIBE_TAG_OPTION_INPUT_SCHEMA, LIST_PORTFOLIOS_INPUT_SCHEMA,
};

use crate::harness::{
    call, create_portfolio, emulator, error_code, is_catalog_id, message, tag_map,
    DEFAULT_REGION, HARNESS_ACCESS_KEY_ID, TARGET_PREFIX,
};

/// The value the `smithy-protocol` header carries on this route ("the `smithy-protocol`
/// header … MUST be set to `rpc-v2-cbor`", that protocol's specification).
const CBOR_PROTOCOL: &str = "rpc-v2-cbor";

/// The media type a CBOR request and its answer carry.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// The member a failure's envelope names the error's class under, on either format.
const TYPE_MEMBER: &str = "__type";

/// The members an error's sentence arrives under: this service's modelled error shapes
/// declare `Message`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), and the framework's
/// own `ValidationException` declares `message`. Both are read, as the JSON side's reader
/// ([`message`]) reads both, so a case is never decided by which of the two answered.
const MESSAGE_MEMBERS: [&str; 2] = ["Message", "message"];

/// One answer of the CBOR route, exactly as answered: its status, the two headers that
/// declare the format, and the body bytes.
#[derive(Debug)]
struct CborAnswer {
    status: u16,
    content_type: Option<String>,
    protocol: Option<String>,
    body: Bytes,
}

/// Call `operation` over the CBOR route.
///
/// The bytes are passed through untouched — encoding and decoding them is the caller's,
/// since only the caller knows the shape — and the SigV4 credential scope is what routes
/// the request to the `servicecatalog` signing name, exactly as it does on the JSON route
/// the rest of this crate's suites drive; the harness verifies no signature.
async fn cbor_call(router: &MultiServiceRouter, operation: &str, body: Bytes) -> CborAnswer {
    let request = ::http::Request::builder()
        .method("POST")
        // `TARGET_PREFIX` is the service shape's local name, which is the `{Service}`
        // label of this protocol's `/service/{Service}/operation/{Operation}` as much as
        // it is the `X-Amz-Target` prefix of the JSON route.
        .uri(format!("/service/{TARGET_PREFIX}/operation/{operation}"))
        .header("content-type", CBOR_MEDIA_TYPE)
        .header("accept", CBOR_MEDIA_TYPE)
        .header("smithy-protocol", CBOR_PROTOCOL)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={HARNESS_ACCESS_KEY_ID}/20260101/{DEFAULT_REGION}/\
                 servicecatalog/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(body)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let content_type = header("content-type");
    let protocol = header("smithy-protocol");
    CborAnswer {
        status,
        content_type,
        protocol,
        body: response.into_body(),
    }
}

/// `input` as the CBOR request body a client sends: written through the protocol's own
/// codec against the operation's input schema, so nothing here encodes CBOR by hand.
fn request_body<I: SerializableStruct>(input_schema: &Schema, input: &I) -> Bytes {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(input_schema, input)
        .expect("the input shape serializes");
    Bytes::from(serializer.finish())
}

/// A successful answer decoded against its own output schema, as a client decodes one.
fn decoded<O: DeserializableStruct>(case: &str, answered: &CborAnswer) -> O {
    assert_eq!(
        answered.status, 200,
        "case {case:?}: the call must succeed on the CBOR route, got {} with {:?}",
        answered.status, answered.body
    );
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&answered.body);
    O::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!("case {case:?}: the answer must decode against its output schema, got {error}")
    })
}

/// The TEXT entries of the CBOR map `body` carries, in the order the map carries them.
///
/// Read with the maintained CBOR decoder rather than against a schema, because the members
/// of an answered ERROR map are not known from any shape: the class is named under a
/// member no error shape declares. An item that is not text is read past, so a map
/// carrying a member that is not a string still reads.
fn error_entries(case: &str, body: &[u8]) -> Vec<(String, String)> {
    fn entries(body: &[u8]) -> Option<Vec<(String, String)>> {
        let mut decoder = minicbor::Decoder::new(body);
        // A map of a declared length, or an indefinite one closed by a break item: an
        // encoder is free to write either.
        let declared = decoder.map().ok()?;
        let mut entries = Vec::new();
        let mut read = 0_u64;
        loop {
            match declared {
                Some(count) if read == count => break,
                None if matches!(decoder.datatype().ok()?, minicbor::data::Type::Break) => {
                    decoder.skip().ok()?;
                    break;
                }
                _ => {}
            }
            let key = decoder.str().ok()?.to_string();
            if matches!(decoder.datatype().ok()?, minicbor::data::Type::String) {
                entries.push((key, decoder.str().ok()?.to_string()));
            } else {
                decoder.skip().ok()?;
            }
            read = read.saturating_add(1);
        }
        Some(entries)
    }

    entries(body)
        .unwrap_or_else(|| panic!("case {case:?}: the failure must answer a CBOR map, got {body:?}"))
}

/// The value of one entry of a CBOR map.
fn entry(entries: &[(String, String)], member: &str) -> Option<String> {
    entries
        .iter()
        .find(|(key, _)| key == member)
        .map(|(_, value)| value.clone())
}

/// The sentence a CBOR failure states, under whichever of [`MESSAGE_MEMBERS`] the
/// answering shape declares.
fn cbor_message(entries: &[(String, String)]) -> Option<String> {
    MESSAGE_MEMBERS
        .iter()
        .find_map(|member| entry(entries, member))
}

/// The class a CBOR failure names, as the JSON route's [`error_code`] reads the same
/// member of the JSON envelope: the last segment of `__type`.
fn cbor_error_code(entries: &[(String, String)]) -> String {
    entry(entries, TYPE_MEMBER)
        .map(|named| {
            named
                .split(['#', ':'])
                .next_back()
                .unwrap_or(&named)
                .to_string()
        })
        .unwrap_or_else(|| format!("<no {TYPE_MEMBER} in {entries:?}>"))
}

/// One portfolio row: what the create carries beyond the members every create must.
struct PortfolioCase {
    case: &'static str,
    display_name: &'static str,
    provider: &'static str,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// Rows differing along the OPTIONAL members a `CreatePortfolio` carries — the members
/// whose presence and count the envelope has to carry faithfully either way.
const PORTFOLIO_CASES: &[PortfolioCase] = &[
    PortfolioCase {
        case: "no description and no tag",
        display_name: "cbor-bare",
        provider: "acme",
        description: None,
        tags: &[],
    },
    PortfolioCase {
        case: "a description and one tag",
        display_name: "cbor-described",
        provider: "globex",
        description: Some("written over the CBOR route"),
        tags: &[("team", "platform")],
    },
    PortfolioCase {
        case: "a description and several tags",
        display_name: "cbor-tagged",
        provider: "initech",
        description: Some("several tags, one envelope"),
        tags: &[("cost-centre", "42"), ("team", "platform"), ("tier", "gold")],
    },
];

/// `pairs` as the modelled `Tags` list of a CBOR request.
fn modelled_tags(pairs: &[(&str, &str)]) -> Option<Vec<Tag>> {
    (!pairs.is_empty()).then(|| {
        pairs
            .iter()
            .map(|(key, value)| Tag {
                key: (*key).to_string(),
                value: (*value).to_string(),
            })
            .collect()
    })
}

/// The epoch milliseconds a JSON timestamp carries, which `awsJson1_1` renders as epoch
/// seconds with a fractional part.
fn json_millis(case: &str, reported: &Value) -> i64 {
    let seconds = reported.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected an epoch-seconds timestamp, got {reported}")
    });
    (seconds * 1_000.0).round() as i64
}

/// Assert the two envelopes reported ONE instant.
///
/// Both formats carry a timestamp as epoch seconds — `awsJson1_1` as a decimal number
/// with a fractional part, `rpcv2Cbor` as the tag-1 epoch-seconds value, which the codec
/// writes as a binary float. Decoding that float back into whole seconds and nanoseconds
/// truncates, so the CBOR form can read one millisecond earlier than the decimal one for
/// the SAME stored instant (observed: `…702.771` on the JSON route against `…702.770`
/// decoded from CBOR). The instants are therefore compared at the resolution the service
/// reports — the millisecond `reported_time` truncates to — with that one-millisecond
/// decode loss allowed and nothing wider.
fn assert_same_instant(case: &str, over_json: i64, over_cbor: i64) {
    assert!(
        over_json.abs_diff(over_cbor) <= 1,
        "case {case:?}: expected the instant the CBOR route reported ({over_cbor} ms), got \
         {over_json} ms"
    );
}

/// RULE: a create and the reads of what it wrote are served on the CBOR route, and the
/// resource they report is the one in the store — which the JSON route reads back as the
/// same resource, down to the instant it was created at.
#[tokio::test]
async fn a_portfolio_written_on_the_cbor_route_reads_back_on_both_routes() {
    let router = emulator();

    for (index, row) in PORTFOLIO_CASES.iter().enumerate() {
        let case = row.case;
        let answered = cbor_call(
            &router,
            "CreatePortfolio",
            request_body(
                CREATE_PORTFOLIO_INPUT_SCHEMA,
                &CreatePortfolioInput {
                    display_name: row.display_name.to_string(),
                    provider_name: row.provider.to_string(),
                    description: row.description.map(str::to_string),
                    tags: modelled_tags(row.tags),
                    idempotency_token: format!("cbor-create-{index:02}"),
                    ..CreatePortfolioInput::default()
                },
            ),
        )
        .await;
        assert_eq!(
            answered.content_type.as_deref(),
            Some(CBOR_MEDIA_TYPE),
            "case {case:?}: an answer of the CBOR route carries the CBOR media type, got {:?}",
            answered.content_type
        );
        assert_eq!(
            answered.protocol.as_deref(),
            Some(CBOR_PROTOCOL),
            "case {case:?}: an answer of the CBOR route declares its protocol, got {:?}",
            answered.protocol
        );
        let created: CreatePortfolioOutput = decoded(case, &answered);
        let detail = created.portfolio_detail.clone().unwrap_or_else(|| {
            panic!("case {case:?}: the create must report the portfolio, got {created:?}")
        });
        let portfolio_id = detail
            .id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the create must report an id, got {detail:?}"));
        assert!(
            is_catalog_id("port", &portfolio_id),
            "case {case:?}: expected a minted portfolio id, got {portfolio_id:?}"
        );
        assert_eq!(
            detail.display_name.as_deref(),
            Some(row.display_name),
            "case {case:?}: expected the display name the create named, got {detail:?}"
        );
        assert_eq!(
            detail.description.as_deref(),
            row.description,
            "case {case:?}: expected the description the create carried, got {detail:?}"
        );
        let created_millis = detail
            .created_time
            .and_then(|stamp| stamp.to_millis().ok())
            .unwrap_or_else(|| {
                panic!("case {case:?}: the create must report its instant, got {detail:?}")
            });

        // The read of what that create wrote, on the SAME route.
        let answered = cbor_call(
            &router,
            "DescribePortfolio",
            request_body(
                DESCRIBE_PORTFOLIO_INPUT_SCHEMA,
                &DescribePortfolioInput {
                    id: portfolio_id.clone(),
                    ..DescribePortfolioInput::default()
                },
            ),
        )
        .await;
        let read: DescribePortfolioOutput = decoded(case, &answered);
        let read_detail = read.portfolio_detail.clone().unwrap_or_else(|| {
            panic!("case {case:?}: the read must report the portfolio, got {read:?}")
        });
        assert_eq!(
            read_detail.id.as_deref(),
            Some(portfolio_id.as_str()),
            "case {case:?}: expected the id the create minted, got {read_detail:?}"
        );
        assert_eq!(
            read_detail.provider_name.as_deref(),
            Some(row.provider),
            "case {case:?}: expected the provider the create named, got {read_detail:?}"
        );
        assert_eq!(
            read_detail
                .created_time
                .and_then(|stamp| stamp.to_millis().ok()),
            Some(created_millis),
            "case {case:?}: expected the instant the create reported, got {read_detail:?}"
        );
        let read_tags: Vec<(String, String)> = read
            .tags
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|tag| (tag.key, tag.value))
            .collect();
        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            read_tags, expected_tags,
            "case {case:?}: expected the tags the create carried, got {read_tags:?}"
        );

        // The SAME resource through the OTHER envelope: one store behind two formats.
        let (status, over_json) = call(
            &router,
            "DescribePortfolio",
            &json!({"Id": portfolio_id.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the JSON route must read the same portfolio, got {status} with \
             {over_json}"
        );
        assert_eq!(
            over_json["PortfolioDetail"]["Id"].as_str(),
            Some(portfolio_id.as_str()),
            "case {case:?}: the JSON route must report the same id, got {over_json}"
        );
        assert_eq!(
            over_json["PortfolioDetail"]["ARN"].as_str(),
            read_detail.arn.as_deref(),
            "case {case:?}: the JSON route must report the same ARN, got {over_json}"
        );
        assert_same_instant(
            case,
            json_millis(case, &over_json["PortfolioDetail"]["CreatedTime"]),
            created_millis,
        );
        assert_eq!(
            tag_map(case, &over_json["Tags"]),
            expected_tags.iter().cloned().collect(),
            "case {case:?}: the JSON route must report the same tags, got {over_json}"
        );
    }

    // And the list on the CBOR route holds every portfolio either route wrote.
    let answered = cbor_call(
        &router,
        "ListPortfolios",
        request_body(
            LIST_PORTFOLIOS_INPUT_SCHEMA,
            &ListPortfoliosInput::default(),
        ),
    )
    .await;
    let listed: ListPortfoliosOutput = decoded("the whole list", &answered);
    let names: Vec<String> = listed
        .portfolio_details
        .unwrap_or_default()
        .into_iter()
        .filter_map(|detail| detail.display_name)
        .collect();
    for row in PORTFOLIO_CASES {
        assert!(
            names.iter().any(|name| name == row.display_name),
            "case {:?}: expected {:?} among the listed portfolios, got {names:?}",
            row.case,
            row.display_name
        );
    }
}

/// RULE, for the other DIRECTION: a resource written on the JSON route reads back on the
/// CBOR route, and the page token one route minted is honoured by the other — the token is
/// the service's own opaque resume key, not a property of the envelope that carried it.
#[tokio::test]
async fn a_portfolio_written_on_the_json_route_reads_back_on_the_cbor_route() {
    let router = emulator();

    // Rows differ along the tags the JSON create carried, which the CBOR read has to
    // report back unchanged.
    let cases: &[PortfolioCase] = &[
        PortfolioCase {
            case: "written on the JSON route with no tag",
            display_name: "json-bare",
            provider: "acme",
            description: None,
            tags: &[],
        },
        PortfolioCase {
            case: "written on the JSON route with tags",
            display_name: "json-tagged",
            provider: "acme",
            // The JSON seeder every suite of this crate shares carries no description,
            // so neither row does and both must read back without one.
            description: None,
            tags: &[("team", "platform"), ("tier", "silver")],
        },
    ];

    let mut written = Vec::new();
    for (index, row) in cases.iter().enumerate() {
        let (case, display_name, tags) = (row.case, row.display_name, row.tags);
        let portfolio_id = create_portfolio(
            &router,
            display_name,
            row.provider,
            tags,
            &format!("json-create-{index:02}"),
        )
        .await;
        written.push(portfolio_id.clone());

        let answered = cbor_call(
            &router,
            "DescribePortfolio",
            request_body(
                DESCRIBE_PORTFOLIO_INPUT_SCHEMA,
                &DescribePortfolioInput {
                    id: portfolio_id.clone(),
                    ..DescribePortfolioInput::default()
                },
            ),
        )
        .await;
        let read: DescribePortfolioOutput = decoded(case, &answered);
        let detail = read.portfolio_detail.clone().unwrap_or_else(|| {
            panic!("case {case:?}: the CBOR read must report the portfolio, got {read:?}")
        });
        assert_eq!(
            detail.display_name.as_deref(),
            Some(display_name),
            "case {case:?}: expected the display name the JSON create named, got {detail:?}"
        );
        assert_eq!(
            detail.description.as_deref(),
            row.description,
            "case {case:?}: expected the description the JSON create carried, got {detail:?}"
        );
        let read_tags: Vec<(String, String)> = read
            .tags
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|tag| (tag.key, tag.value))
            .collect();
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            read_tags, expected_tags,
            "case {case:?}: expected the tags the JSON create carried, got {read_tags:?}"
        );
    }

    // One portfolio per page over the CBOR route: the token the first page answered
    // resumes the second, and the two pages together are the two portfolios written.
    let first = cbor_call(
        &router,
        "ListPortfolios",
        request_body(
            LIST_PORTFOLIOS_INPUT_SCHEMA,
            &ListPortfoliosInput {
                page_size: 1,
                ..ListPortfoliosInput::default()
            },
        ),
    )
    .await;
    let first_page: ListPortfoliosOutput = decoded("the first page", &first);
    let token = first_page.next_page_token.clone().unwrap_or_else(|| {
        panic!("case {:?}: a page of one of two must carry a resume token, got {first_page:?}", "the first page")
    });
    let second = cbor_call(
        &router,
        "ListPortfolios",
        request_body(
            LIST_PORTFOLIOS_INPUT_SCHEMA,
            &ListPortfoliosInput {
                page_size: 1,
                page_token: Some(token.clone()),
                ..ListPortfoliosInput::default()
            },
        ),
    )
    .await;
    let second_page: ListPortfoliosOutput = decoded("the second page", &second);
    let paged: Vec<String> = first_page
        .portfolio_details
        .unwrap_or_default()
        .into_iter()
        .chain(second_page.portfolio_details.unwrap_or_default())
        .filter_map(|detail| detail.id)
        .collect();
    assert_eq!(
        paged, written,
        "case {:?}: expected the two portfolios the JSON route wrote, got {paged:?}",
        "two pages of one"
    );
}

/// RULE, for the failing half: a failure of a CBOR call is the SAME failure the JSON route
/// answers — the status, the class and its sentence — carried in the CBOR map a client of
/// that route decodes.
///
/// Rows differ along the LAYER that answers: two handlers' modelled misses, a handler's
/// modelled refusal of a duplicate (whose first half was written on the other route), and
/// the boundary refusal a value outside a modelled constraint earns before any handler
/// runs.
#[tokio::test]
async fn a_failure_on_the_cbor_route_is_the_failure_the_json_route_answers() {
    let router = emulator();

    // A read of a portfolio nothing holds: the handler's own modelled miss.
    let missing = "port-000000000000";
    assert_same_failure(
        "a read of a portfolio nothing holds",
        "ResourceNotFoundException",
        400,
        &router,
        ("DescribePortfolio", json!({"Id": missing})),
        (
            "DescribePortfolio",
            request_body(
                DESCRIBE_PORTFOLIO_INPUT_SCHEMA,
                &DescribePortfolioInput {
                    id: missing.to_string(),
                    ..DescribePortfolioInput::default()
                },
            ),
        ),
    )
    .await;

    // A read of a TagOption nothing holds: a second resource, a second sentence.
    let missing_option = "tag-000000000000";
    assert_same_failure(
        "a read of a TagOption nothing holds",
        "ResourceNotFoundException",
        400,
        &router,
        ("DescribeTagOption", json!({"Id": missing_option})),
        (
            "DescribeTagOption",
            request_body(
                DESCRIBE_TAG_OPTION_INPUT_SCHEMA,
                &DescribeTagOptionInput {
                    id: missing_option.to_string(),
                },
            ),
        ),
    )
    .await;

    // A value outside a modelled constraint (`Id` is len 1..100), which the boundary
    // validator refuses before any handler runs.
    assert_same_failure(
        "an id shorter than the modelled minimum",
        "ValidationException",
        400,
        &router,
        ("DescribePortfolio", json!({"Id": ""})),
        (
            "DescribePortfolio",
            request_body(
                DESCRIBE_PORTFOLIO_INPUT_SCHEMA,
                &DescribePortfolioInput {
                    id: String::new(),
                    ..DescribePortfolioInput::default()
                },
            ),
        ),
    )
    .await;

    // The pair a TagOption IS, created on the CBOR route: the duplicate the JSON route
    // then earns is the same refusal the CBOR route earns for it.
    let created = cbor_call(
        &router,
        "CreateTagOption",
        request_body(
            CREATE_TAG_OPTION_INPUT_SCHEMA,
            &CreateTagOptionInput {
                key: "environment".to_string(),
                value: "production".to_string(),
            },
        ),
    )
    .await;
    let option: CreateTagOptionOutput = decoded("a TagOption created on the CBOR route", &created);
    let option_id = option
        .tag_option_detail
        .and_then(|detail| detail.id)
        .unwrap_or_else(|| panic!("the create must report the TagOption id, got {created:?}"));
    assert!(
        is_catalog_id("tag", &option_id),
        "a TagOption created on the CBOR route: expected a minted id, got {option_id:?}"
    );
    // And it reads back on the CBOR route as the pair it was created from.
    let read = cbor_call(
        &router,
        "DescribeTagOption",
        request_body(
            DESCRIBE_TAG_OPTION_INPUT_SCHEMA,
            &DescribeTagOptionInput {
                id: option_id.clone(),
            },
        ),
    )
    .await;
    let read: DescribeTagOptionOutput = decoded("a TagOption read on the CBOR route", &read);
    let detail = read.tag_option_detail.clone().unwrap_or_else(|| {
        panic!("a TagOption read on the CBOR route: expected the TagOption, got {read:?}")
    });
    assert_eq!(
        (detail.key.as_deref(), detail.value.as_deref()),
        (Some("environment"), Some("production")),
        "a TagOption read on the CBOR route: expected the pair the create named, got {detail:?}"
    );
    assert_same_failure(
        "the pair a CBOR create already stored",
        "DuplicateResourceException",
        400,
        &router,
        (
            "CreateTagOption",
            json!({"Key": "environment", "Value": "production"}),
        ),
        (
            "CreateTagOption",
            request_body(
                CREATE_TAG_OPTION_INPUT_SCHEMA,
                &CreateTagOptionInput {
                    key: "environment".to_string(),
                    value: "production".to_string(),
                },
            ),
        ),
    )
    .await;
}

/// Assert that the two routes answered ONE failure — the same status, the same class and
/// the same sentence — and that the class is the one expected for it, so a pair that
/// agreed on something else cannot pass.
async fn assert_same_failure(
    case: &str,
    expected_class: &str,
    expected_status: u16,
    router: &MultiServiceRouter,
    over_json: (&str, Value),
    over_cbor: (&str, Bytes),
) {
    let (json_operation, json_request) = over_json;
    let (json_status, json_body) = call(router, json_operation, &json_request).await;
    assert_eq!(
        json_status, expected_status,
        "case {case:?}: expected the JSON route to answer {expected_status}, got {json_status} \
         with {json_body}"
    );
    assert_eq!(
        error_code(&json_body),
        expected_class,
        "case {case:?}: expected the JSON route to answer {expected_class}, got {json_body}"
    );

    let (cbor_operation, cbor_request) = over_cbor;
    let answered = cbor_call(router, cbor_operation, cbor_request).await;
    assert_eq!(
        answered.status, json_status,
        "case {case:?}: expected the status the JSON route answered ({json_status}), got {:?}",
        answered
    );
    let entries = error_entries(case, &answered.body);
    assert_eq!(
        cbor_error_code(&entries),
        error_code(&json_body),
        "case {case:?}: expected the class the JSON route named ({}), got {entries:?}",
        error_code(&json_body)
    );
    assert_eq!(
        cbor_message(&entries).as_deref(),
        Some(message(&json_body).as_str()),
        "case {case:?}: expected the sentence the JSON route stated ({:?}), got {entries:?}",
        message(&json_body)
    );
}
