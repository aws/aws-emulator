// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Polly lexicon lifecycle, speech synthesis and the voice
//! catalog, driven over HTTP through the same router the emulator serves with.
//!
//! Each test states the RULE it is checking and then checks it over a TABLE of inputs
//! that span the class the rule covers — several PLS documents rather than one, every
//! `OutputFormat` rather than mp3, every documented sample rate rather than the
//! default. The single request an issue happens to quote is one row.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names, so the ARNs the service mints
/// are predictable.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator with Polly mounted on it, driven exactly as the router drives it.
///
/// `pub(crate)` because the whole `tests/all/` tree is ONE test crate: the other
/// suites drive the service through this harness rather than standing up a second one.
pub(crate) struct Polly {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Polly {
    pub(crate) fn new() -> Self {
        Self::mounting(false)
    }

    /// One emulator with Polly AND Amazon S3 mounted on the SAME engine, exactly as the
    /// assembled emulator mounts them.
    ///
    /// The bucket a synthesis task writes its output to is a resource of THAT service, and
    /// whether the object arrived is a fact only it can answer, so a suite about the output
    /// of a completed task needs one to reach; [`Polly::new`] deliberately leaves it
    /// unmounted, which is the "the destination bucket could not be written" case.
    pub(crate) fn with_s3() -> Self {
        Self::mounting(true)
    }

    /// One emulator with Polly and Amazon S3 mounted on it, SERVED at `base` — the value a
    /// router served on a data port learns from the core, which a router that is not served
    /// is told instead ([`MultiServiceRouter::set_url_base`]).
    ///
    /// A suite about an address this service ANNOUNCES needs that base to vary: the address
    /// this process is served at is what such an announcement carries for a caller that
    /// states no address of this emulator, and a router that is not served reports one and
    /// the same unserved base for every row.
    pub(crate) fn with_s3_served_at(base: &str) -> Self {
        let mut polly = Self::mounting(true);
        polly.router.set_url_base(
            smithy_server_runtime::EndpointUrl::parse(base)
                .unwrap_or_else(|| panic!("{base:?} must parse as an endpoint base")),
        );
        polly
    }

    /// Stand up the router, with Amazon S3 beside Polly when `s3` says so.
    fn mounting(s3: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests exercise handler behaviour, not SigV4; the `Authorization`
        // header below is still what routes the request and names the region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        if s3 {
            router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
                Arc::clone(&engine),
            ));
        }
        Self { router, engine }
    }

    /// A handle to this emulator for driving it through an AWS SDK client — the same
    /// in-process seam a handler reaches another mounted service through.
    pub(crate) fn callback(&self) -> smithy_server_runtime::EmbeddedEmulator {
        smithy_server_runtime::EmbeddedEmulator::from_router(self.router.clone())
    }

    /// An Amazon S3 client for `region` over this emulator's own in-process callback — the
    /// very seam, and the very client construction
    /// ([`service_common::s3_client::s3_client`]), a completed synthesis task writes its
    /// output object through.
    pub(crate) fn s3(&self, region: &str) -> ::aws_sdk_s3::Client {
        ::service_common::s3_client::s3_client(&self.callback(), region)
    }

    /// The engine this emulator serves from, for a suite that has to seed state a
    /// still-scaffolded operation would otherwise have to create.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    pub(crate) async fn call(
        &self,
        method: &str,
        path: &str,
        body: &str,
    ) -> http::Response<Bytes> {
        self.call_in_region(REGION, method, path, body).await
    }

    /// The same call signed for `region`, for a suite about a Region-scoped resource.
    ///
    /// The credential scope is what names the Region, exactly as it does for a real
    /// SDK client configured for one.
    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: &str,
    ) -> http::Response<Bytes> {
        self.call_bytes_in_region(
            region,
            method,
            path,
            &[("content-type", "application/json")],
            body.as_bytes().to_vec(),
        )
        .await
    }

    /// One call whose body is BYTES and whose request headers the caller chooses —
    /// the shape an operation that binds its input to headers and its payload to an
    /// event stream takes.
    ///
    /// Every call above goes through this one, so there is a single definition of how
    /// a test request is signed and addressed.
    pub(crate) async fn call_bytes_in_region(
        &self,
        region: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> http::Response<Bytes> {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://polly.{region}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/polly/aws4_request, \
                     SignedHeaders=host, Signature=0"
                ),
            );
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request
            .body(Bytes::from(body))
            .expect("a well-formed test request");
        self.router.dispatch(request).await
    }

    pub(crate) async fn put_lexicon(&self, name: &str, content: &str) -> http::Response<Bytes> {
        self.put_lexicon_in(REGION, name, content).await
    }

    /// `PutLexicon` signed for `region`.
    pub(crate) async fn put_lexicon_in(
        &self,
        region: &str,
        name: &str,
        content: &str,
    ) -> http::Response<Bytes> {
        let body = serde_json::json!({ "Content": content }).to_string();
        self.call_in_region(region, "PUT", &format!("/v1/lexicons/{name}"), &body)
            .await
    }

    pub(crate) async fn get_lexicon(&self, name: &str) -> http::Response<Bytes> {
        self.get_lexicon_in(REGION, name).await
    }

    /// `GetLexicon` signed for `region`.
    pub(crate) async fn get_lexicon_in(&self, region: &str, name: &str) -> http::Response<Bytes> {
        self.call_in_region(region, "GET", &format!("/v1/lexicons/{name}"), "")
            .await
    }

    pub(crate) async fn delete_lexicon(&self, name: &str) -> http::Response<Bytes> {
        self.delete_lexicon_in(REGION, name).await
    }

    /// `DeleteLexicon` signed for `region`.
    pub(crate) async fn delete_lexicon_in(
        &self,
        region: &str,
        name: &str,
    ) -> http::Response<Bytes> {
        self.call_in_region(region, "DELETE", &format!("/v1/lexicons/{name}"), "")
            .await
    }

    pub(crate) async fn list_lexicons(&self, query: &str) -> http::Response<Bytes> {
        self.list_lexicons_in(REGION, query).await
    }

    /// `ListLexicons` signed for `region`.
    pub(crate) async fn list_lexicons_in(
        &self,
        region: &str,
        query: &str,
    ) -> http::Response<Bytes> {
        self.call_in_region(region, "GET", &format!("/v1/lexicons{query}"), "")
            .await
    }

    pub(crate) async fn describe_voices(&self, query: &str) -> http::Response<Bytes> {
        self.call("GET", &format!("/v1/voices{query}"), "").await
    }

    pub(crate) async fn synthesize(&self, request: &serde_json::Value) -> http::Response<Bytes> {
        self.call("POST", "/v1/speech", &request.to_string()).await
    }

    /// `SynthesizeSpeech` signed for `region`.
    pub(crate) async fn synthesize_in(
        &self,
        region: &str,
        request: &serde_json::Value,
    ) -> http::Response<Bytes> {
        self.call_in_region(region, "POST", "/v1/speech", &request.to_string())
            .await
    }
}

/// The JSON body of a response, for the operations that return one.
pub(crate) fn json(response: &http::Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or_else(|e| {
        panic!(
            "expected a JSON body, got {:?}: {e}",
            String::from_utf8_lossy(response.body())
        )
    })
}

/// The modeled error code a failure response carries (`__type` on restJson1).
pub(crate) fn error_code(response: &http::Response<Bytes>) -> String {
    json(response)
        .get("__type")
        .and_then(|t| t.as_str())
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

pub(crate) fn header(response: &http::Response<Bytes>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// A PLS lexicon with `lexemes` lexemes in `alphabet` for `language`.
///
/// `pub(crate)` for the same reason as the harness: every suite builds its documents
/// here rather than keeping a second definition of a valid one.
pub(crate) fn pls(alphabet: &str, language: &str, lexemes: usize) -> String {
    let body: String = (0..lexemes)
        .map(|i| {
            format!(
                "  <lexeme><grapheme>G{i}</grapheme><alias>Grapheme {i}</alias></lexeme>\n"
            )
        })
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <lexicon version=\"1.0\" \
         xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
         alphabet=\"{alphabet}\" xml:lang=\"{language}\">\n{body}</lexicon>"
    )
}

/// A valid PLS lexicon exactly `total` characters long (padding goes in a grapheme).
fn pls_of_size(total: usize) -> String {
    const PREFIX: &str = "<lexicon version=\"1.0\" \
                          xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
                          alphabet=\"ipa\" xml:lang=\"en-US\"><lexeme><grapheme>";
    const SUFFIX: &str = "</grapheme></lexeme></lexicon>";
    let padding = total.saturating_sub(PREFIX.len() + SUFFIX.len());
    format!("{PREFIX}{}{SUFFIX}", "x".repeat(padding))
}

/// RULE: a lexicon is stored under its name, read back verbatim with the attributes
/// derived from its own document, replaced in place by a later put of the same name,
/// and gone from every read after a delete.
#[tokio::test]
async fn a_lexicon_round_trips_through_state_and_is_replaced_in_place() {
    let polly = Polly::new();
    let first = pls("ipa", "en-US", 2);

    let put = polly.put_lexicon("RoundTrip", &first).await;
    assert_eq!(put.status(), 200, "PutLexicon: body {:?}", json(&put));

    let got = polly.get_lexicon("RoundTrip").await;
    assert_eq!(got.status(), 200, "GetLexicon: body {:?}", json(&got));
    let body = json(&got);
    assert_eq!(
        body["Lexicon"]["Content"].as_str(),
        Some(first.as_str()),
        "GetLexicon must return the content that was put, character for character"
    );
    assert_eq!(body["Lexicon"]["Name"].as_str(), Some("RoundTrip"));
    let attributes = &body["LexiconAttributes"];
    assert_eq!(attributes["Alphabet"].as_str(), Some("ipa"));
    assert_eq!(attributes["LanguageCode"].as_str(), Some("en-US"));
    assert_eq!(attributes["LexemesCount"].as_u64(), Some(2));
    assert_eq!(
        attributes["Size"].as_u64(),
        Some(first.chars().count() as u64),
        "Size is the lexicon's size in characters"
    );
    assert_eq!(
        attributes["LexiconArn"].as_str(),
        Some(format!("arn:aws:polly:{REGION}:{ACCOUNT}:lexicon/RoundTrip").as_str()),
        "the ARN is minted for the request's region and the emulator's account"
    );
    let first_modified = attributes["LastModified"]
        .as_f64()
        .expect("LastModified is a timestamp");

    let listed = polly.list_lexicons("").await;
    assert_eq!(listed.status(), 200);
    assert_eq!(
        listed_names(&json(&listed)),
        BTreeSet::from(["RoundTrip".to_string()]),
        "ListLexicons must report the lexicon that was put"
    );

    // Replacing: same name, different document. One entry, new attributes, and a
    // LastModified that has not gone backwards.
    let second = pls("x-sampa", "de-DE", 5);
    assert_eq!(polly.put_lexicon("RoundTrip", &second).await.status(), 200);
    let replaced = json(&polly.get_lexicon("RoundTrip").await);
    assert_eq!(
        replaced["Lexicon"]["Content"].as_str(),
        Some(second.as_str()),
        "a re-put must overwrite the stored content"
    );
    assert_eq!(replaced["LexiconAttributes"]["Alphabet"].as_str(), Some("x-sampa"));
    assert_eq!(replaced["LexiconAttributes"]["LanguageCode"].as_str(), Some("de-DE"));
    assert_eq!(replaced["LexiconAttributes"]["LexemesCount"].as_u64(), Some(5));
    let second_modified = replaced["LexiconAttributes"]["LastModified"]
        .as_f64()
        .expect("LastModified is a timestamp");
    assert!(
        second_modified >= first_modified,
        "LastModified moved backwards: {second_modified} < {first_modified}"
    );
    assert_eq!(
        listed_names(&json(&polly.list_lexicons("").await)).len(),
        1,
        "a re-put must not duplicate the entry"
    );

    let deleted = polly.delete_lexicon("RoundTrip").await;
    assert_eq!(deleted.status(), 200, "DeleteLexicon: {:?}", json(&deleted));
    let after_delete = polly.get_lexicon("RoundTrip").await;
    assert_eq!(
        (after_delete.status().as_u16(), error_code(&after_delete)),
        (404, "LexiconNotFoundException".to_string()),
        "GetLexicon after a delete must be a 404"
    );
    assert!(
        listed_names(&json(&polly.list_lexicons("").await)).is_empty(),
        "ListLexicons must not report a deleted lexicon"
    );
    let delete_again = polly.delete_lexicon("RoundTrip").await;
    assert_eq!(
        (delete_again.status().as_u16(), error_code(&delete_again)),
        (404, "LexiconNotFoundException".to_string()),
        "deleting an absent lexicon must be a 404"
    );
}

/// The names `ListLexicons` reported.
pub(crate) fn listed_names(body: &serde_json::Value) -> BTreeSet<String> {
    listed_names_in_order(body).into_iter().collect()
}

/// The names `ListLexicons` reported, in the order the response listed them.
///
/// [`listed_names`] answers the SET a response reported; this answers the SEQUENCE,
/// for a rule about the order the list is served in.
pub(crate) fn listed_names_in_order(body: &serde_json::Value) -> Vec<String> {
    body["Lexicons"]
        .as_array()
        .expect("Lexicons is a list")
        .iter()
        .map(|entry| {
            entry["Name"]
                .as_str()
                .expect("every description names its lexicon")
                .to_string()
        })
        .collect()
}

/// RULE: every `LexiconAttributes` value but the ARN and the timestamp is read out of
/// the PLS document itself, so a document with a different alphabet, language, lexeme
/// count or length reports exactly those differences.
#[tokio::test]
async fn lexicon_attributes_are_derived_from_the_document() {
    let polly = Polly::new();
    // Each alphabet is one the model's `UnsupportedPlsAlphabetException` documentation
    // lists; `en` is the bare primary subtag `LexiconAttributes$LanguageCode`
    // documents ("A lexicon with a language code such as \"en\" ...").
    let table = [
        ("Ipa", "ipa", "en-US", 1usize),
        ("Sampa", "x-sampa", "en-GB", 3),
        ("Pinyin", "x-amazon-pinyin", "cmn-CN", 7),
        ("Jyutping", "x-amazon-jyutping", "yue-CN", 2),
        ("Kana", "x-amazon-pron-kana", "ja-JP", 4),
        ("Yomigana", "x-amazon-yomigana", "ja-JP", 0),
        ("Generic", "ipa", "en", 2),
        ("Arabic", "ipa", "arb", 1),
    ];
    for (name, alphabet, language, lexemes) in table {
        let content = pls(alphabet, language, lexemes);
        let put = polly.put_lexicon(name, &content).await;
        assert_eq!(
            put.status(),
            200,
            "case {name:?}: PutLexicon failed with {:?}",
            json(&put)
        );
        let attributes = json(&polly.get_lexicon(name).await)["LexiconAttributes"].clone();
        assert_eq!(
            (
                attributes["Alphabet"].as_str(),
                attributes["LanguageCode"].as_str(),
                attributes["LexemesCount"].as_u64(),
                attributes["Size"].as_u64(),
            ),
            (
                Some(alphabet),
                Some(language),
                Some(lexemes as u64),
                Some(content.chars().count() as u64),
            ),
            "case {name:?}: attributes do not match the document, got {attributes:?}"
        );
    }
    assert_eq!(
        listed_names(&json(&polly.list_lexicons("").await)).len(),
        table.len(),
        "every lexicon put must be listed, and no others"
    );
}

/// RULE: a `Content` that is not a PLS lexicon Amazon Polly can read — including one
/// declaring an alphabet it does not support — is `InvalidLexiconException`, one
/// declaring an unsupported language is `UnsupportedPlsLanguageException`, and
/// nothing is stored either way.
#[tokio::test]
async fn an_unusable_pls_document_is_refused_with_its_modeled_error() {
    let polly = Polly::new();
    let table = [
        ("not xml", "this is not xml at all <", "InvalidLexiconException"),
        ("other root", "<speak>hello</speak>", "InvalidLexiconException"),
        ("empty", "", "InvalidLexiconException"),
        (
            "unknown alphabet",
            "<lexicon version=\"1.0\" \
             xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
             alphabet=\"x-klingon\" xml:lang=\"en-US\"/>",
            "InvalidLexiconException",
        ),
        (
            "unknown language",
            "<lexicon version=\"1.0\" \
             xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
             alphabet=\"ipa\" xml:lang=\"xx-ZZ\"/>",
            "UnsupportedPlsLanguageException",
        ),
    ];
    for (case, content, expected) in table {
        let response = polly.put_lexicon("Refused", content).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, expected.to_string()),
            "case {case:?}: wrong refusal, body {:?}",
            json(&response)
        );
    }
    assert!(
        listed_names(&json(&polly.list_lexicons("").await)).is_empty(),
        "a refused PutLexicon must store nothing"
    );
}

/// RULE: the lexicon store is bounded — a lexicon past the published size quota is
/// `LexiconSizeExceededException`, an insert past the published count quota is
/// `MaxLexiconsNumberExceededException`, and replacing a name already stored is
/// neither.
#[tokio::test]
async fn the_lexicon_store_is_bounded() {
    let polly = Polly::new();

    // The size cap, exercised either side of the boundary.
    for (case, total, expect_ok) in [
        ("at the cap", 4_000usize, true),
        ("one past the cap", 4_001, false),
        ("far past the cap", 40_000, false),
    ] {
        let content = pls_of_size(total);
        assert_eq!(
            content.chars().count(),
            total,
            "case {case:?}: the test document is the wrong size"
        );
        let response = polly.put_lexicon("Sized", &content).await;
        if expect_ok {
            assert_eq!(
                response.status(),
                200,
                "case {case:?}: expected success, body {:?}",
                json(&response)
            );
        } else {
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "LexiconSizeExceededException".to_string()),
                "case {case:?}: expected a refusal"
            );
        }
    }

    // The count cap. `Sized` above already occupies one slot.
    let small = pls("ipa", "en-US", 1);
    for index in 0..99 {
        let name = format!("Count{index:02}");
        assert_eq!(
            polly.put_lexicon(&name, &small).await.status(),
            200,
            "PutLexicon {name} must succeed below the count cap"
        );
    }
    let over = polly.put_lexicon("OneTooMany", &small).await;
    assert_eq!(
        (over.status().as_u16(), error_code(&over)),
        (400, "MaxLexiconsNumberExceededException".to_string()),
        "a new name past the count cap must be refused"
    );
    // Replacing an existing name adds no entry, so the cap does not refuse it.
    assert_eq!(
        polly.put_lexicon("Count00", &small).await.status(),
        200,
        "replacing a stored lexicon must still succeed at the count cap"
    );
}

/// RULE: `ListLexicons` serves the Region's whole store in name order in ONE response
/// with no `NextToken`, and a token this service could not have minted is
/// `InvalidNextTokenException`.
#[tokio::test]
async fn list_lexicons_serves_the_whole_store_and_refuses_a_foreign_token() {
    let polly = Polly::new();
    let content = pls("ipa", "en-US", 1);
    let total = 25;
    for index in 0..total {
        let name = format!("Lex{index:02}");
        assert_eq!(
            polly.put_lexicon(&name, &content).await.status(),
            200,
            "PutLexicon {name} failed"
        );
    }

    let response = polly.list_lexicons("").await;
    assert_eq!(
        response.status(),
        200,
        "ListLexicons failed, body {:?}",
        json(&response)
    );
    let body = json(&response);
    let seen: Vec<String> = listed_names_in_order(&body);
    let expected: Vec<String> = (0..total).map(|index| format!("Lex{index:02}")).collect();
    assert_eq!(
        seen, expected,
        "one call must report the whole store, in name order"
    );
    assert_eq!(
        body["NextToken"].as_str(),
        None,
        "the response is not truncated, so no NextToken is owed, got {:?}",
        body["NextToken"]
    );

    for bad in ["not-a-token", "zzzz", "ff"] {
        let response = polly.list_lexicons(&format!("?NextToken={bad}")).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "InvalidNextTokenException".to_string()),
            "token {bad:?}: expected a refusal, body {:?}",
            json(&response)
        );
    }
}

/// RULE: `SynthesizeSpeech` answers with the `OutputFormat`'s documented media type, a
/// non-empty stream in that encoding, and the billed character count of `Text`.
#[tokio::test]
async fn synthesize_speech_answers_every_output_format() {
    let polly = Polly::new();
    let text = "Hello world, this is a sentence.";
    // The mapping is `SynthesizeSpeechOutput$ContentType`'s own documentation; the
    // signature is the format's real framing.
    let table: [(&str, &str, Option<&[u8]>); 6] = [
        ("mp3", "audio/mpeg", Some(&[0xFF])),
        ("ogg_opus", "audio/ogg", Some(b"OggS")),
        ("ogg_vorbis", "audio/ogg", Some(b"OggS")),
        ("pcm", "audio/pcm", None),
        ("mulaw", "audio/mulaw", None),
        ("alaw", "audio/alaw", None),
    ];
    for (format, content_type, signature) in table {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": text,
                "VoiceId": "Joanna",
                "OutputFormat": format,
            }))
            .await;
        assert_eq!(
            response.status(),
            200,
            "format {format:?}: body {:?}",
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            header(&response, "content-type"),
            content_type,
            "format {format:?}: wrong Content-Type"
        );
        assert_eq!(
            header(&response, "x-amzn-requestcharacters"),
            text.chars().count().to_string(),
            "format {format:?}: wrong RequestCharacters"
        );
        assert!(
            !response.body().is_empty(),
            "format {format:?}: the audio stream must not be empty"
        );
        if let Some(signature) = signature {
            assert_eq!(
                &response.body()[..signature.len()],
                signature,
                "format {format:?}: the stream does not start with the format's framing"
            );
        }
    }
}

/// RULE: every modelled `OutputFormat` is SERVED — none of them answers an error the
/// operation does not model — and the Ogg formats carry a real stream of the encoding
/// they name, whichever documented sample rate is asked for.
#[tokio::test]
async fn every_modelled_output_format_is_served() {
    let polly = Polly::new();
    // (format, the codec name its stream carries, the rates it documents that this row
    // drives). `ogg_vorbis` documents six rates and `ogg_opus` one.
    let table: [(&str, &[u8], &[&str]); 2] = [
        ("ogg_vorbis", b"vorbis", &["8000", "22050", "48000"]),
        ("ogg_opus", b"OpusHead", &["48000"]),
    ];
    for (format, codec, rates) in table {
        for rate in rates {
            let request = serde_json::json!({
                "Text": "Testing output format content types.",
                "VoiceId": "Joanna",
                "OutputFormat": format,
                "SampleRate": rate,
            });
            let response = polly.synthesize(&request).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "request {request:?}: expected audio, got body {:?}",
                String::from_utf8_lossy(response.body())
            );
            assert_eq!(
                header(&response, "content-type"),
                "audio/ogg",
                "request {request:?}: wrong Content-Type"
            );
            let body = response.body();
            assert_eq!(
                &body[..4],
                b"OggS",
                "request {request:?}: the stream must be Ogg-framed"
            );
            assert!(
                body.windows(codec.len()).any(|window| window == codec),
                "request {request:?}: the stream must name its codec ({:?})",
                String::from_utf8_lossy(codec)
            );
        }
    }
    // A request that names no rate at all is served too, at the format's default.
    for format in ["ogg_vorbis", "ogg_opus"] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": format,
            }))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "format {format:?} with no SampleRate: body {:?}",
            String::from_utf8_lossy(response.body())
        );
    }
}

/// RULE: the stream's length follows the text, so more text means more audio for every
/// encoding whose length carries the signal.
#[tokio::test]
async fn a_longer_text_yields_a_longer_stream() {
    let polly = Polly::new();
    let short = "Hi.";
    let long = "Hi. ".repeat(100);
    for format in ["mp3", "ogg_opus", "ogg_vorbis", "pcm", "mulaw", "alaw"] {
        let mut lengths = Vec::new();
        for text in [short, long.as_str()] {
            let response = polly
                .synthesize(&serde_json::json!({
                    "Text": text,
                    "VoiceId": "Matthew",
                    "OutputFormat": format,
                }))
                .await;
            assert_eq!(response.status(), 200, "format {format:?}: synthesis failed");
            lengths.push(response.body().len());
        }
        assert!(
            lengths[1] > lengths[0],
            "format {format:?}: expected a longer stream for longer text, got {lengths:?}"
        );
    }
}

/// RULE: `SynthesizeSpeech` resolves every name in `LexiconNames` against the lexicon
/// store, so a name that is not stored is `LexiconNotFoundException` (404) — including
/// a name that was stored and then deleted.
#[tokio::test]
async fn synthesis_resolves_lexicon_names_against_the_store() {
    let polly = Polly::new();
    let content = pls("ipa", "en-US", 1);
    for name in ["LexA", "LexB"] {
        assert_eq!(polly.put_lexicon(name, &content).await.status(), 200);
    }

    let request = |names: serde_json::Value| {
        serde_json::json!({
            "Text": "hello world",
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
            "LexiconNames": names,
        })
    };

    for stored in [
        serde_json::json!([]),
        serde_json::json!(["LexA"]),
        serde_json::json!(["LexA", "LexB"]),
    ] {
        let response = polly.synthesize(&request(stored.clone())).await;
        assert_eq!(
            response.status(),
            200,
            "lexicons {stored:?}: expected success, body {:?}",
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            header(&response, "x-amzn-requestcharacters"),
            "11",
            "lexicons {stored:?}: wrong RequestCharacters"
        );
    }

    for missing in [
        serde_json::json!(["NoSuchLex"]),
        serde_json::json!(["LexA", "NoSuchLex"]),
        serde_json::json!(["lexa"]),
    ] {
        let response = polly.synthesize(&request(missing.clone())).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (404, "LexiconNotFoundException".to_string()),
            "lexicons {missing:?}: expected a 404"
        );
    }

    // Deleting a lexicon makes synthesis that names it fail: the store is the only
    // source of truth here.
    assert_eq!(polly.delete_lexicon("LexA").await.status(), 200);
    let after_delete = polly.synthesize(&request(serde_json::json!(["LexA"]))).await;
    assert_eq!(
        (after_delete.status().as_u16(), error_code(&after_delete)),
        (404, "LexiconNotFoundException".to_string()),
        "a deleted lexicon must no longer resolve"
    );
}

/// RULE: a `SampleRate` is accepted exactly when the chosen `OutputFormat` documents
/// it, and refused with `InvalidSampleRateException` otherwise.
#[tokio::test]
async fn sample_rates_follow_the_documented_values_per_format() {
    let polly = Polly::new();
    // Verbatim from `SynthesizeSpeechInput$SampleRate`'s documentation.
    let table: [(&str, &[&str], &[&str]); 5] = [
        (
            "mp3",
            &["8000", "16000", "22050", "24000", "44100", "48000"],
            &["11025", "0", "96000", "not-a-number"],
        ),
        // `ogg_vorbis` has no accepted-rate row: the rate is still validated (the
        // refusals below prove it), but a valid rate reaches the 501 the format
        // answers, which `an_unencodable_output_format_is_not_dressed_up_as_success`
        // covers.
        ("ogg_vorbis", &[], &["11025", "32000"]),
        ("ogg_opus", &["48000"], &["8000", "24000"]),
        ("pcm", &["8000", "16000"], &["22050", "24000"]),
        ("mulaw", &["8000"], &["16000", "22050"]),
    ];
    for (format, valid, invalid) in table {
        for rate in valid {
            let response = polly
                .synthesize(&serde_json::json!({
                    "Text": "hello",
                    "VoiceId": "Joanna",
                    "OutputFormat": format,
                    "SampleRate": rate,
                }))
                .await;
            assert_eq!(
                response.status(),
                200,
                "format {format:?} rate {rate:?}: expected success, body {:?}",
                String::from_utf8_lossy(response.body())
            );
        }
        for rate in invalid {
            let response = polly
                .synthesize(&serde_json::json!({
                    "Text": "hello",
                    "VoiceId": "Joanna",
                    "OutputFormat": format,
                    "SampleRate": rate,
                }))
                .await;
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "InvalidSampleRateException".to_string()),
                "format {format:?} rate {rate:?}: expected a refusal"
            );
        }
    }
}

/// RULE: `RequestCharacters` is the billed character count — every character of plain
/// text, and for SSML the text content only — and text past the documented limits is
/// `TextLengthExceededException`.
#[tokio::test]
async fn request_characters_counts_billed_characters() {
    let polly = Polly::new();
    let table: [(&str, &str, &str, u64); 5] = [
        ("ascii", "hello world", "text", 11),
        ("empty-ish", ".", "text", 1),
        ("non-ascii", "ünïcödé", "text", 7),
        ("ssml", "<speak>hello world</speak>", "ssml", 11),
        (
            "ssml with tags",
            "<speak>one <break time=\"1s\"/>two</speak>",
            "ssml",
            7,
        ),
    ];
    for (case, text, text_type, expected) in table {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": text,
                "TextType": text_type,
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
            }))
            .await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: body {:?}",
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            header(&response, "x-amzn-requestcharacters"),
            expected.to_string(),
            "case {case:?}: wrong RequestCharacters"
        );
    }

    // 6000 total characters is the documented limit; 6001 is past it.
    let too_long = "a".repeat(6001);
    let response = polly
        .synthesize(&serde_json::json!({
            "Text": too_long,
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
        }))
        .await;
    assert_eq!(
        (response.status().as_u16(), error_code(&response)),
        (400, "TextLengthExceededException".to_string()),
        "text past the total limit must be refused"
    );

    // Malformed SSML is the SSML error, not a 500 and not a silent success.
    let bad_ssml = polly
        .synthesize(&serde_json::json!({
            "Text": "<speak>unclosed",
            "TextType": "ssml",
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
        }))
        .await;
    assert_eq!(
        (bad_ssml.status().as_u16(), error_code(&bad_ssml)),
        (400, "InvalidSsmlException".to_string()),
        "malformed SSML must be refused"
    );
}

/// RULE: speech marks ride the `json` stream only — asking for them with an audio
/// format is `MarksNotSupportedForFormatException` — and the marks a `json` request
/// gets back are computed from the text.
#[tokio::test]
async fn speech_marks_are_json_only_and_computed_from_the_text() {
    let polly = Polly::new();
    for format in ["mp3", "pcm", "ogg_opus"] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "hello world",
                "VoiceId": "Joanna",
                "OutputFormat": format,
                "SpeechMarkTypes": ["word"],
            }))
            .await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "MarksNotSupportedForFormatException".to_string()),
            "format {format:?}: speech marks must be refused"
        );
    }

    let response = polly
        .synthesize(&serde_json::json!({
            "Text": "Hi there. Go!",
            "VoiceId": "Joanna",
            "OutputFormat": "json",
            "SpeechMarkTypes": ["sentence", "word"],
        }))
        .await;
    assert_eq!(response.status(), 200, "json synthesis must succeed");
    assert_eq!(
        header(&response, "content-type"),
        "application/x-json-stream",
        "json output carries the speech-mark media type"
    );
    let marks: Vec<serde_json::Value> = String::from_utf8_lossy(response.body())
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is a JSON object"))
        .collect();
    let rendered: Vec<(String, u64, u64, String)> = marks
        .iter()
        .map(|mark| {
            (
                mark["type"].as_str().unwrap_or_default().to_string(),
                mark["start"].as_u64().unwrap_or_default(),
                mark["end"].as_u64().unwrap_or_default(),
                mark["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        rendered,
        vec![
            ("sentence".to_string(), 0, 9, "Hi there.".to_string()),
            ("word".to_string(), 0, 2, "Hi".to_string()),
            ("word".to_string(), 3, 8, "there".to_string()),
            ("sentence".to_string(), 10, 13, "Go!".to_string()),
            ("word".to_string(), 10, 12, "Go".to_string()),
        ],
        "the marks must be the text's own sentences and words"
    );

    // Asking for no mark type asks for no marks, so the stream is empty — not a
    // refusal Amazon Polly does not declare.
    let no_types = polly
        .synthesize(&serde_json::json!({
            "Text": "Hi there. Go!",
            "VoiceId": "Joanna",
            "OutputFormat": "json",
        }))
        .await;
    assert_eq!(no_types.status(), 200, "json without mark types must succeed");
    assert!(
        no_types.body().is_empty(),
        "no mark type means no marks, got {:?}",
        String::from_utf8_lossy(no_types.body())
    );

    // Marks over SSML input report offsets into the text the caller sent — the markup
    // included, because that is the string the offsets index into.
    for mark_types in [
        serde_json::json!(["word"]),
        serde_json::json!(["sentence"]),
        serde_json::json!(["sentence", "word"]),
    ] {
        const SSML: &str = "<speak>one <break time=\"1s\"/>two</speak>";
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": SSML,
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "json",
                "SpeechMarkTypes": mark_types,
            }))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "mark types {mark_types:?} over ssml input: expected marks, body {:?}",
            String::from_utf8_lossy(response.body())
        );
        let marks: Vec<serde_json::Value> = String::from_utf8_lossy(response.body())
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line is a JSON object"))
            .collect();
        assert!(
            !marks.is_empty(),
            "mark types {mark_types:?} over ssml input: expected at least one mark"
        );
        for mark in &marks {
            let start = mark["start"].as_u64().unwrap_or_default() as usize;
            let end = mark["end"].as_u64().unwrap_or_default() as usize;
            let value = mark["value"].as_str().unwrap_or_default();
            assert!(
                end <= SSML.len() && start < end,
                "mark {mark} must span a stretch of the text it was sent"
            );
            assert_eq!(
                value,
                SSML[start..end].trim(),
                "mark {mark} must carry the text its offsets cover"
            );
            if mark["type"].as_str() == Some("word") {
                assert!(
                    !value.contains('<') && !value.contains('>'),
                    "word mark {mark} must cover a word of the text content, not markup"
                );
            }
        }
    }

    // One mark type at a time yields only that type.
    for (requested, expected) in [("sentence", 2usize), ("word", 3)] {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": "Hi there. Go!",
                "VoiceId": "Joanna",
                "OutputFormat": "json",
                "SpeechMarkTypes": [requested],
            }))
            .await;
        assert_eq!(response.status(), 200, "mark type {requested:?}: failed");
        let body = String::from_utf8_lossy(response.body()).to_string();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines.len(),
            expected,
            "mark type {requested:?}: expected {expected} marks, got {lines:?}"
        );
        for line in lines {
            assert!(
                line.contains(&format!("\"type\":\"{requested}\"")),
                "mark type {requested:?}: a line reports another type: {line}"
            );
        }
    }
}

/// RULE: `Engine` is checked against the voice the request names, as the catalog
/// `DescribeVoices` serves records it — so an engine the voice does not support is the
/// request-shape refusal `ValidationException`. `LanguageCode` is NOT such a check: it
/// selects which language a bilingual voice speaks and states no requirement on the
/// voice, so a bilingual voice's additional language and a language the voice does not
/// speak at all are both synthesized with the voice the request named.
#[tokio::test]
async fn synthesis_checks_the_voice_against_the_catalog() {
    let polly = Polly::new();
    // Each row is read off the seeded catalog, which DescribeVoices also serves.
    let table: [(&str, &str, Option<&str>, Option<&str>); 7] = [
        // (voice, engine, language, expected error code or None for success)
        ("Joanna", "neural", None, None),
        ("Joanna", "standard", None, None),
        ("Geraint", "standard", None, None),
        ("Geraint", "neural", None, Some("ValidationException")),
        ("Kevin", "standard", None, Some("ValidationException")),
        // Aditi is the bilingual voice the model's own documentation names: en-IN by
        // default, hi-IN as an additional language.
        ("Aditi", "standard", Some("hi-IN"), None),
        // A language the voice does not speak is served too: the member only selects a
        // bilingual voice's language (see `crate::language_code_and_voice`).
        ("Joanna", "neural", Some("hi-IN"), None),
    ];
    for (voice, engine, language, expected) in table {
        let mut request = serde_json::json!({
            "Text": "hello world",
            "VoiceId": voice,
            "Engine": engine,
            "OutputFormat": "mp3",
        });
        if let Some(language) = language {
            request["LanguageCode"] = serde_json::json!(language);
        }
        let response = polly.synthesize(&request).await;
        match expected {
            None => assert_eq!(
                response.status(),
                200,
                "voice {voice:?} engine {engine:?} language {language:?}: expected success, \
                 body {:?}",
                String::from_utf8_lossy(response.body())
            ),
            Some(code) => assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, code.to_string()),
                "voice {voice:?} engine {engine:?} language {language:?}: wrong refusal"
            ),
        }
    }

    // The engine the catalog reports for a voice is what synthesis accepts, for every
    // voice DescribeVoices lists under that engine.
    for engine in ["standard", "neural"] {
        for voice in voices(&polly, &format!("?Engine={engine}")).await.iter().take(5) {
            let id = voice["Id"].as_str().expect("every voice has an id");
            let response = polly
                .synthesize(&serde_json::json!({
                    "Text": "hi",
                    "VoiceId": id,
                    "Engine": engine,
                    "OutputFormat": "mp3",
                }))
                .await;
            assert_eq!(
                response.status(),
                200,
                "voice {id:?} is listed for engine {engine:?} but synthesis refused it: {:?}",
                String::from_utf8_lossy(response.body())
            );
        }
    }
}

/// RULE: `DescribeVoices` serves the catalog narrowed by its query parameters —
/// `Engine` keeps voices that support it, `LanguageCode` keeps voices that speak it,
/// and a bilingual voice appears for a secondary language only when
/// `IncludeAdditionalLanguageCodes` is set.
#[tokio::test]
async fn describe_voices_filters_the_catalog() {
    let polly = Polly::new();

    let all = voices(&polly, "").await;
    assert!(
        all.len() > 20,
        "the seeded catalog must carry a real set of voices, got {}",
        all.len()
    );
    for voice in &all {
        for field in ["Id", "Name", "Gender", "LanguageCode", "LanguageName"] {
            assert!(
                voice[field].as_str().is_some_and(|v| !v.is_empty()),
                "voice {:?}: {field} is missing",
                voice["Id"]
            );
        }
        assert!(
            voice["SupportedEngines"]
                .as_array()
                .is_some_and(|e| !e.is_empty()),
            "voice {:?}: SupportedEngines is empty",
            voice["Id"]
        );
    }

    // A language filter narrows to that language, and only to it.
    for language in ["en-US", "de-DE", "ja-JP", "arb"] {
        let filtered = voices(&polly, &format!("?LanguageCode={language}")).await;
        assert!(
            !filtered.is_empty(),
            "language {language:?}: the catalog must carry voices for it"
        );
        assert!(
            filtered.len() < all.len(),
            "language {language:?}: the filter did not narrow the list"
        );
        for voice in &filtered {
            assert_eq!(
                voice["LanguageCode"].as_str(),
                Some(language),
                "language {language:?}: voice {:?} does not speak it",
                voice["Id"]
            );
        }
    }

    // An engine filter keeps exactly the voices the unfiltered catalog reports that
    // engine for — so an engine every voice is offered on would keep them all and an
    // engine none is offered on keeps none, and nothing in between is silently widened.
    for engine in ["standard", "neural", "generative", "long-form"] {
        let filtered = voices(&polly, &format!("?Engine={engine}")).await;
        let expected: BTreeSet<String> = all
            .iter()
            .filter(|voice| {
                voice["SupportedEngines"]
                    .as_array()
                    .is_some_and(|engines| engines.iter().any(|e| e.as_str() == Some(engine)))
            })
            .map(|voice| voice["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            ids(&filtered),
            expected,
            "engine {engine:?}: the filter kept the wrong voices, {} reported for {} \
             expected",
            filtered.len(),
            expected.len()
        );
        assert!(
            filtered.len() < all.len(),
            "engine {engine:?}: the filter did not narrow the list"
        );
        for voice in &filtered {
            let engines: Vec<&str> = voice["SupportedEngines"]
                .as_array()
                .expect("SupportedEngines is a list")
                .iter()
                .filter_map(|e| e.as_str())
                .collect();
            assert!(
                engines.contains(&engine),
                "engine {engine:?}: voice {:?} supports only {engines:?}",
                voice["Id"]
            );
        }
    }

    // Both filters together narrow further than either alone.
    let en_us = voices(&polly, "?LanguageCode=en-US").await;
    let en_us_standard = voices(&polly, "?LanguageCode=en-US&Engine=standard").await;
    assert!(
        !en_us_standard.is_empty() && en_us_standard.len() < en_us.len(),
        "combining the filters must narrow further: {} of {}",
        en_us_standard.len(),
        en_us.len()
    );

    // A query no voice satisfies is an empty list, not the whole catalog. Every
    // modelled language has a voice, so the empty case is a language crossed with an
    // engine none of its voices supports (Welsh is a standard-only language).
    let none = voices(&polly, "?LanguageCode=cy-GB&Engine=generative").await;
    assert!(
        none.is_empty(),
        "a query with no matching voices must answer an empty list, got {none:?}"
    );

    // `Aditi` speaks hi-IN as an additional language (the model documents exactly
    // this), so it appears for hi-IN only with IncludeAdditionalLanguageCodes.
    let primary = ids(&voices(&polly, "?LanguageCode=hi-IN").await);
    let with_additional = ids(&voices(
        &polly,
        "?LanguageCode=hi-IN&IncludeAdditionalLanguageCodes=true",
    )
    .await);
    assert!(
        !primary.contains("Aditi"),
        "a bilingual voice must not appear for its secondary language by default: {primary:?}"
    );
    assert!(
        with_additional.contains("Aditi"),
        "IncludeAdditionalLanguageCodes must add the bilingual voice: {with_additional:?}"
    );
    assert!(
        primary.is_subset(&with_additional),
        "IncludeAdditionalLanguageCodes must only widen the list"
    );

    // A token this service did not mint is refused.
    let bad = polly.describe_voices("?NextToken=not-a-token").await;
    assert_eq!(
        (bad.status().as_u16(), error_code(&bad)),
        (400, "InvalidNextTokenException".to_string()),
        "a foreign NextToken must be refused"
    );
}

/// Every voice a `DescribeVoices` query reports, from the one response that carries
/// them all.
///
/// `pub(crate)` so the catalog suite reads the catalog the same way this one does.
/// The read asserts the listing is not truncated, so every caller of this helper also
/// pins the rule that this operation sends no `NextToken`.
pub(crate) async fn voices(polly: &Polly, query: &str) -> Vec<serde_json::Value> {
    let response = polly.describe_voices(query).await;
    assert_eq!(
        response.status(),
        200,
        "DescribeVoices{query}: body {:?}",
        String::from_utf8_lossy(response.body())
    );
    let body = json(&response);
    assert_eq!(
        body["NextToken"].as_str(),
        None,
        "case {query:?}: the whole catalog is in the response, so no NextToken is owed, \
         got {:?}",
        body["NextToken"]
    );
    body["Voices"].as_array().expect("Voices is a list").clone()
}

pub(crate) fn ids(voices: &[serde_json::Value]) -> BTreeSet<String> {
    voices
        .iter()
        .map(|voice| voice["Id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: every operation in this slice reads the one shared store, so a lexicon put
/// through one request is visible to every later request — this is the whole point of
/// the engine seam.
#[tokio::test]
async fn state_written_by_one_request_is_visible_to_the_next() {
    let polly = Polly::new();
    let content = pls("ipa", "en-US", 1);
    assert_eq!(polly.put_lexicon("Shared", &content).await.status(), 200);

    // A different operation, a different request: it must see the write.
    assert!(
        listed_names(&json(&polly.list_lexicons("").await)).contains("Shared"),
        "ListLexicons must see what PutLexicon wrote"
    );
    assert_eq!(
        polly
            .synthesize(&serde_json::json!({
                "Text": "hello",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
                "LexiconNames": ["Shared"],
            }))
            .await
            .status(),
        200,
        "SynthesizeSpeech must see what PutLexicon wrote"
    );

    // And a second emulator shares nothing with the first.
    let other = Polly::new();
    assert!(
        listed_names(&json(&other.list_lexicons("").await)).is_empty(),
        "a separate emulator must not see another's lexicons"
    );
}
