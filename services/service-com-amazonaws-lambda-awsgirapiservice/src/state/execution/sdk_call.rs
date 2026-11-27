// SPDX-License-Identifier: CC0-1.0

//! What every half of the SDK call seam that is bound by an AWS SDK CLIENT shares: how one
//! call's input document is read, what the ANSWER itself carried, and how the called
//! service's own refusal is handed back to the guest.
//!
//! A service whose protocol carries the input document as the request body needs none of
//! this: one code path builds, signs and dispatches every operation of every such service
//! ([`super::aws_api`]). A service whose protocol binds each operation differently — a
//! form-encoded request and an XML answer, a path and an XML document — is reached through
//! the SDK CLIENT this crate holds for it, over the same in-process callback transport, and
//! each of those halves then has the same three jobs: read the keyword arguments the handler
//! passed as the operation's own input members, record what the HTTP answer carried (an SDK
//! client reports output members and not the answer they were read out of), and report a
//! refusal as the code, message and status the service itself answered.
//!
//! Those three are here so that a seam for one such service states only WHICH operations it
//! binds and which members each carries. Nothing here knows any service's operations: the
//! service is named only in the sentences a refusal carries, so a handler is told which
//! call was not made and why.

use ::std::collections::HashMap;
use ::std::sync::{Arc, Mutex, PoisonError};

use ::serde_json::{Map, Value};

use super::aws_api::{Answer, Refusal, MAX_RESPONSE_BYTES};

/// The provider name a signing identity built from the invocation's own credential carries:
/// it names this runtime rather than any AWS credential source, and never travels on the
/// wire.
pub(super) const PROVIDER_NAME: &str = "aws-lambda-execution-role";

/// The header an AWS answer carries the id of the request it answers in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The input document one call carries: the keyword arguments the handler passed, read as
/// the operation's own input members.
pub(super) struct Input {
    /// The service the call is made to, as a refusal names it.
    service: &'static str,
    /// The members the document carries.
    members: Map<String, Value>,
}

impl Input {
    /// The document `body` carries, or a call not made when it is not an object at all.
    pub(super) fn read(service: &'static str, body: &str) -> Result<Self, Refusal> {
        let parsed: Value = ::serde_json::from_str(body).map_err(|error| {
            Refusal::NotMade(format!("the call's parameters are not a document: {error}"))
        })?;
        match parsed {
            Value::Object(members) => Ok(Self { service, members }),
            _ => Err(Refusal::NotMade(
                "the call's parameters must be the operation's own input members".to_string(),
            )),
        }
    }

    /// A nested document, read as the members of the structure it is.
    fn nested(&self, members: &Map<String, Value>) -> Self {
        Self {
            service: self.service,
            members: members.clone(),
        }
    }

    /// Refuse to send a call carrying a member this binding does not carry, naming it: a
    /// member silently dropped would make a request other than the one asked for.
    pub(super) fn only(&self, bound: &[&str]) -> Result<(), Refusal> {
        let mut unbound: Vec<&str> = self
            .members
            .keys()
            .map(String::as_str)
            .filter(|name| !bound.contains(name))
            .collect();
        unbound.sort_unstable();
        if unbound.is_empty() {
            return Ok(());
        }
        Err(Refusal::NotMade(format!(
            "the AWS SDK this runtime supplies does not carry the {} parameter(s) of this {} \
             call; the ones it carries are {}",
            unbound.join(", "),
            self.service,
            bound.join(", ")
        )))
    }

    /// A string member, or `None` where the document carries none.
    pub(super) fn text(&self, name: &str) -> Result<Option<String>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(other) => Err(Refusal::NotMade(format!(
                "the call's {name} must be text, got {other}"
            ))),
        }
    }

    /// A string member the operation requires.
    pub(super) fn required_text(&self, name: &str) -> Result<String, Refusal> {
        self.text(name)?.ok_or_else(|| self.missing(name))
    }

    /// A boolean member, as the model's own `boolean` shape carries it.
    pub(super) fn boolean(&self, name: &str) -> Result<Option<bool>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(other) => Err(Refusal::NotMade(format!(
                "the call's {name} must be true or false, got {other}"
            ))),
        }
    }

    /// An INTEGER member, as the model's own `integer` shape carries it: a value outside what
    /// that shape can hold stops the call rather than being sent truncated.
    ///
    /// Read from EITHER spelling an SDK takes such a member in: the number itself, and the
    /// decimal text of it. A whole number an operation binds to its request's query string
    /// travels as decimal text, which is why the SDKs model several of those members as text
    /// and a handler passes `"1"` where another passes `1` — the two ask for the very same
    /// call, so neither spelling stops it. What still stops the call is a value that is no
    /// whole number this shape can hold, whichever spelling it arrived in: that one would have
    /// to be sent as something other than what the handler asked for.
    ///
    /// Stated for every member this reader is asked for, deliberately: it is handed a member's
    /// NAME and not its shape, so which members one SDK spells as text is not a fact available
    /// here — and a value read from either spelling carries the same number into the same
    /// call, so the one rule costs no fidelity on a member an SDK spells as a number.
    pub(super) fn integer(&self, name: &str) -> Result<Option<i32>, Refusal> {
        let carried = match self.members.get(name) {
            None | Some(Value::Null) => return Ok(None),
            Some(spelled @ (Value::Number(_) | Value::String(_))) => spelled,
            Some(other) => {
                return Err(Refusal::NotMade(format!(
                    "the call's {name} must be a number, got {other}"
                )))
            }
        };
        whole_number(carried).map(Some).ok_or_else(|| {
            Refusal::NotMade(format!(
                "the call's {name} is not a whole number this operation can carry, got {carried}"
            ))
        })
    }

    /// A member whose shape is a LIST OF STRINGS, read as the values it holds; an empty list
    /// where the document carries the member as none at all.
    pub(super) fn text_list(&self, name: &str) -> Result<Vec<String>, Refusal> {
        let elements = match self.members.get(name) {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(Value::Array(elements)) => elements,
            Some(other) => {
                return Err(Refusal::NotMade(format!(
                    "the call's {name} must be a list, got {other}"
                )))
            }
        };
        elements
            .iter()
            .map(|element| match element {
                Value::String(text) => Ok(text.clone()),
                other => Err(Refusal::NotMade(format!(
                    "every entry of the call's {name} must be text, got {other}"
                ))),
            })
            .collect()
    }

    /// The same, for a list-of-strings member the operation REQUIRES: a document carrying
    /// none of it stops the call rather than sending the required member emptied.
    pub(super) fn required_text_list(&self, name: &str) -> Result<Vec<String>, Refusal> {
        if !self.members.get(name).is_some_and(|value| !value.is_null()) {
            return Err(self.missing(name));
        }
        self.text_list(name)
    }

    /// A member whose shape is a STRUCTURE, read as the members it holds; `None` where the
    /// document carries none.
    pub(super) fn structure(&self, name: &str) -> Result<Option<Self>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Object(members)) => Ok(Some(self.nested(members))),
            Some(other) => Err(Refusal::NotMade(format!(
                "the call's {name} must be a structure, got {other}"
            ))),
        }
    }

    /// A member whose shape is a LIST OF STRUCTURES, read as one document per entry; an empty
    /// list where the document carries the member as none at all.
    pub(super) fn structures(&self, name: &str) -> Result<Vec<Self>, Refusal> {
        let elements = match self.members.get(name) {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(Value::Array(elements)) => elements,
            Some(other) => {
                return Err(Refusal::NotMade(format!(
                    "the call's {name} must be a list, got {other}"
                )))
            }
        };
        elements
            .iter()
            .map(|element| match element {
                Value::Object(members) => Ok(self.nested(members)),
                other => Err(Refusal::NotMade(format!(
                    "every entry of the call's {name} must be a structure, got {other}"
                ))),
            })
            .collect()
    }

    /// The same, for a list-of-structures member the operation REQUIRES.
    pub(super) fn required_structures(&self, name: &str) -> Result<Vec<Self>, Refusal> {
        if !self.members.get(name).is_some_and(|value| !value.is_null()) {
            return Err(self.missing(name));
        }
        self.structures(name)
    }

    /// A member whose shape is a MAP OF STRINGS TO STRINGS, or `None` where the document
    /// carries none.
    pub(super) fn text_map(&self, name: &str) -> Result<Option<HashMap<String, String>>, Refusal> {
        let entries = match self.members.get(name) {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::Object(entries)) => entries,
            Some(other) => {
                return Err(Refusal::NotMade(format!(
                    "the call's {name} must be a map of text, got {other}"
                )))
            }
        };
        let mut held = HashMap::with_capacity(entries.len());
        for (key, value) in entries {
            let Value::String(text) = value else {
                return Err(Refusal::NotMade(format!(
                    "the call's {name}[{key}] must be text, got {value}"
                )));
            };
            held.insert(key.clone(), text.clone());
        }
        Ok(Some(held))
    }

    /// A member whose shape is a MAP OF STRINGS TO STRUCTURES, read as one document per
    /// entry under the key it arrived at; an empty list where the document carries none.
    pub(super) fn structure_map(&self, name: &str) -> Result<Vec<(String, Self)>, Refusal> {
        let entries = match self.members.get(name) {
            None | Some(Value::Null) => return Ok(Vec::new()),
            Some(Value::Object(entries)) => entries,
            Some(other) => {
                return Err(Refusal::NotMade(format!(
                    "the call's {name} must be a map, got {other}"
                )))
            }
        };
        entries
            .iter()
            .map(|(key, value)| match value {
                Value::Object(members) => Ok((key.clone(), self.nested(members))),
                other => Err(Refusal::NotMade(format!(
                    "the call's {name}[{key}] must be a structure, got {other}"
                ))),
            })
            .collect()
    }

    /// A member carrying BYTES, which travel as the base64 text a JSON protocol carries a
    /// blob as — which is how the SDK the guest calls hands over the bytes a handler passed
    /// ([`super::aws_sdk_guest`]).
    pub(super) fn blob(&self, name: &str) -> Result<Option<Vec<u8>>, Refusal> {
        let Some(text) = self.text(name)? else {
            return Ok(None);
        };
        ::base64::Engine::decode(&::base64::engine::general_purpose::STANDARD, text)
            .map(Some)
            .map_err(|error| {
                Refusal::NotMade(format!(
                    "the call's {name} did not arrive as the bytes it carries: {error}"
                ))
            })
    }

    /// The refusal a call missing a member the operation requires is reported as.
    fn missing(&self, name: &str) -> Refusal {
        Refusal::NotMade(format!(
            "the call carries no {name}, which this {} operation requires",
            self.service
        ))
    }
}

/// The whole number `spelled` carries, in either spelling an SDK takes an `integer` member
/// in: the number itself, or the decimal text of it.
///
/// `None` for a value that is no whole number an `integer` shape can hold — a fraction, a
/// magnitude past the shape's range, or text that spells no number at all — which is what
/// stops the call that would otherwise carry something else.
fn whole_number(spelled: &Value) -> Option<i32> {
    match spelled {
        Value::Number(number) => number.as_i64().and_then(|held| i32::try_from(held).ok()),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// What the ANSWER itself carried, recorded as it arrived rather than assumed: the status the
/// service answered and the id it gave the request.
///
/// An SDK client reports an operation's output members and not the HTTP answer they were read
/// out of, so the one place that answer passes through is read here and nowhere else.
#[derive(Clone)]
pub(super) struct Observed {
    /// The service the call was made to, as a refusal names it.
    service: &'static str,
    /// What the answer carried, once one has arrived.
    carried: Arc<Mutex<Option<Carried>>>,
}

/// The facts of one HTTP answer this seam reports to the guest.
#[derive(Clone)]
struct Carried {
    /// The status the service answered.
    status: u16,
    /// The id it gave the request it answered.
    request_id: Option<String>,
}

/// The name this interceptor is registered under on a client's own configuration.
const OBSERVER_NAME: &str = "lambda-runtime-sdk-answer";

impl ::std::fmt::Debug for Observed {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("Observed").finish_non_exhaustive()
    }
}

impl ::aws_smithy_runtime_api::client::interceptors::Intercept for Observed {
    fn name(&self) -> &'static str {
        OBSERVER_NAME
    }

    fn read_before_deserialization(
        &self,
        context: &::aws_smithy_runtime_api::client::interceptors::context::BeforeDeserializationInterceptorContextRef<'_>,
        _components: &::aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
        _config: &mut ::aws_smithy_types::config_bag::ConfigBag,
    ) -> Result<(), ::aws_smithy_runtime_api::box_error::BoxError> {
        let response = context.response();
        let carried = Carried {
            status: response.status().as_u16(),
            request_id: response
                .headers()
                .get(REQUEST_ID_HEADER)
                .map(str::to_string),
        };
        *self.carried.lock().unwrap_or_else(PoisonError::into_inner) = Some(carried);
        Ok(())
    }
}

impl Observed {
    /// An observer of the answers of one service's calls.
    pub(super) fn of(service: &'static str) -> Self {
        Self {
            service,
            carried: Arc::new(Mutex::new(None)),
        }
    }

    /// What the answer carried, or a call not made where no answer reached this seam at all.
    fn read(&self) -> Result<Carried, Refusal> {
        self.carried
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                Refusal::NotMade(format!(
                    "the call reached no {} answer at all",
                    self.service
                ))
            })
    }

    /// One served answer: the output members `document` holds, under the status and request
    /// id the service itself answered.
    ///
    /// Bounded by [`MAX_RESPONSE_BYTES`], the ceiling every answer of this seam is read back
    /// under: a listing whose page a caller made large is reported as an answer too large to
    /// read rather than handed over whole.
    pub(super) fn answer(&self, document: Map<String, Value>) -> Result<Answer, Refusal> {
        let carried = self.read()?;
        let body = Value::Object(document).to_string();
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(Refusal::ResponseTooLarge);
        }
        Ok(Answer {
            status: carried.status,
            body,
            error_type: None,
            // The services reached through an SDK client here speak their own protocol
            // rather than a query-COMPATIBLE one, so no answer of theirs carries a legacy
            // query error code beside the code it names.
            query_error_code: None,
            request_id: carried.request_id,
            // An answer that is not a refusal names no error shape, so it carries none of one's
            // members.
            error_members: ::std::collections::BTreeMap::new(),
            streams: Vec::new(),
            blobs: Vec::new(),
            // WHERE this answer carries an INSTANT is read where every protocol's is
            // ([`super::aws_api::AwsAccess::call_within`]), from the called operation's own
            // model: the document above states a timestamp as the epoch seconds a JSON
            // protocol carries one as ([`insert_timestamp`]).
            instants: Vec::new(),
            payload_blobs: Vec::new(),
        })
    }

    /// The called service's own REFUSAL, as the answer the guest reads back: the code it
    /// named, the message it worded and the status it answered, so the SDK the guest calls
    /// raises the modelled error class for that code with that message.
    ///
    /// A call that reached no answer — nothing to report a status or a code from — is
    /// reported as a call that was not made, carrying why.
    pub(super) fn refused<E, R>(
        &self,
        error: &::aws_smithy_runtime_api::client::result::SdkError<E, R>,
    ) -> Result<Answer, Refusal>
    where
        E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
        ::aws_smithy_runtime_api::client::result::SdkError<E, R>: ::std::fmt::Display,
    {
        let Some(code) = ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(error)
        else {
            return Err(Refusal::NotMade(format!(
                "the {} call could not be made: {error}",
                self.service
            )));
        };
        let carried = self.read()?;
        let mut document = Map::new();
        // `__type` and `message` are where the SDK the guest calls reads a refusal's code and
        // sentence from, so the answer carries the service's own in both.
        document.insert("__type".to_string(), Value::String(code.to_string()));
        document.insert(
            "message".to_string(),
            Value::String(
                ::aws_smithy_types::error::metadata::ProvideErrorMetadata::message(error)
                    .unwrap_or_default()
                    .to_string(),
            ),
        );
        Ok(Answer {
            status: carried.status,
            body: Value::Object(document).to_string(),
            error_type: Some(code.to_string()),
            query_error_code: None,
            request_id: carried.request_id,
            // The refusal reaches this seam as its code and its sentence alone
            // (`ProvideErrorMetadata`), so no other member of its shape is here to report.
            error_members: ::std::collections::BTreeMap::new(),
            streams: Vec::new(),
            blobs: Vec::new(),
            instants: Vec::new(),
            payload_blobs: Vec::new(),
        })
    }
}

/// Put a string member on an answer's document, where the answer carried one.
pub(super) fn insert_text(document: &mut Map<String, Value>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        document.insert(name.to_string(), Value::String(value.to_string()));
    }
}

/// Put a boolean member on it, the same way: a member the answer did not carry is left off
/// rather than reported as `false`, because "absent" and "false" are different facts about a
/// resource and a handler reads the presence of the member as what the service decided.
pub(super) fn insert_flag(document: &mut Map<String, Value>, name: &str, value: Option<bool>) {
    if let Some(value) = value {
        document.insert(name.to_string(), Value::Bool(value));
    }
}

/// Put a NUMERIC member on it, the same way.
pub(super) fn insert_number(document: &mut Map<String, Value>, name: &str, value: Option<i64>) {
    if let Some(value) = value {
        document.insert(name.to_string(), Value::Number(value.into()));
    }
}

/// Put a FRACTIONAL numeric member on it, the same way: a ratio or a per-unit figure a service
/// reports as a `double`. A value no JSON number can hold (an infinity, a NaN) is left off
/// rather than reported as something else.
pub(super) fn insert_double(document: &mut Map<String, Value>, name: &str, value: Option<f64>) {
    if let Some(number) = value.and_then(::serde_json::Number::from_f64) {
        document.insert(name.to_string(), Value::Number(number));
    }
}

/// Put a TIMESTAMP member on it, as the epoch seconds an AWS JSON protocol carries a timestamp
/// as — which is the form this seam hands every service's timestamps over in.
pub(super) fn insert_timestamp(
    document: &mut Map<String, Value>,
    name: &str,
    value: Option<&::aws_smithy_types::DateTime>,
) {
    if let Some(value) = value {
        document.insert(name.to_string(), epoch_seconds(value));
    }
}

/// One instant as the epoch-second number an AWS JSON protocol carries a timestamp as: the
/// seconds since the epoch, FRACTION INCLUDED, because that protocol carries one as a number
/// that may be fractional and an instant this seam truncated would reach the handler as a
/// different instant from the one the service reported.
///
/// The fraction is carried at MICROSECOND resolution, which is the resolution the SDKs' own
/// instant types hold and the one a parser of this wire reads a sub-second fraction at, so the
/// value a handler reads back and the value a client reads on the wire are the same instant.
fn epoch_seconds(value: &::aws_smithy_types::DateTime) -> Value {
    let micros = i64::from(value.subsec_nanos() / 1_000);
    if micros == 0 {
        return Value::Number(value.secs().into());
    }
    let fractional = value.secs() as f64 + micros as f64 / 1_000_000.0;
    match ::serde_json::Number::from_f64(fractional) {
        Some(number) => Value::Number(number),
        // No instant a service reports is a value a JSON number cannot hold; one that
        // somehow is, is reported as its whole seconds rather than left off the answer.
        None => Value::Number(value.secs().into()),
    }
}

/// Put a list-of-STRUCTURES member on it, each entry rendered by `render`: a list the answer
/// carried no entry of is left off, as every other absent member is.
///
/// Here rather than in one seam because every half of this seam that reads a listing off an
/// answer renders its entries this one way.
pub(super) fn insert_structures<T>(
    document: &mut Map<String, Value>,
    name: &str,
    values: &[T],
    render: impl Fn(&T) -> Value,
) {
    if values.is_empty() {
        return;
    }
    document.insert(
        name.to_string(),
        Value::Array(values.iter().map(render).collect()),
    );
}

/// A list member as an SDK setter takes it: `None` where the call carried none, so a member the
/// handler left out is left out of the REQUEST rather than sent empty.
///
/// Here for the same reason: every half of this seam that carries a list member asks the same
/// question of it.
pub(super) fn some_list<T>(values: Vec<T>) -> Option<Vec<T>> {
    (!values.is_empty()).then_some(values)
}

/// Put a map-of-text member on it, the same way — the attributes an answer reports.
pub(super) fn insert_text_map(
    document: &mut Map<String, Value>,
    name: &str,
    value: Option<&HashMap<String, String>>,
) {
    if let Some(entries) = value {
        let mapped: Map<String, Value> = entries
            .iter()
            .map(|(key, held)| (key.clone(), Value::String(held.clone())))
            .collect();
        document.insert(name.to_string(), Value::Object(mapped));
    }
}

#[cfg(test)]
mod tests {
    use ::serde_json::{Map, Value};

    use super::Input;

    /// The service the refusals below name, as a seam for one service passes it in.
    const SERVICE: &str = "Amazon SNS";

    /// RULE: a call is sent only with the members its operation's binding carries, so a
    /// parameter the binding does not carry is reported as a call that was not made rather
    /// than dropped. The rows differ along the FORM of the document a handler passed: the
    /// bound members alone, one unbound member, several, and the empty document.
    #[test]
    fn a_parameter_the_binding_does_not_carry_stops_the_call() {
        let bound = ["TopicArn", "Message", "Subject"];
        let rows: [(&str, &str, Option<&str>); 5] = [
            ("the bound members alone", r#"{"TopicArn":"t","Message":"m"}"#, None),
            (
                "every bound member",
                r#"{"TopicArn":"t","Message":"m","Subject":"s"}"#,
                None,
            ),
            ("the empty document", "{}", None),
            (
                "one unbound member",
                r#"{"TopicArn":"t","Message":"m","PhoneNumber":"+15550100"}"#,
                Some("PhoneNumber"),
            ),
            (
                "two unbound members",
                r#"{"TopicArn":"t","Zebra":"z","Alpha":1}"#,
                Some("Alpha, Zebra"),
            ),
        ];
        for (case, body, expected) in rows {
            let input = Input::read(SERVICE, body).expect("the row's document is an object");
            let answered = input.only(&bound).err().map(|refusal| format!("{refusal:?}"));
            match expected {
                None => assert!(
                    answered.is_none(),
                    "case {case:?}: expected the call to be sent, got {answered:?}"
                ),
                Some(named) => {
                    let reported = answered.unwrap_or_default();
                    assert!(
                        reported.contains(named),
                        "case {case:?}: expected the refusal to name {named:?}, got {reported:?}"
                    );
                }
            }
        }
    }

    /// Each member is read as the TYPE its own model gives it, and a value of another type
    /// stops the call. The rows differ along that type: text, a boolean, a list of text, a
    /// map of text, a map of structures, and bytes carried as base64.
    #[test]
    fn a_member_is_read_as_the_type_its_model_gives_it() {
        let input = Input::read(
            SERVICE,
            r#"{"TopicArn":"arn:aws:sns:us-east-1:000000000000:orders",
                "ReturnSubscriptionArn":true,
                "TagKeys":["team","owner"],
                "Attributes":{"RawMessageDelivery":"true"},
                "MessageAttributes":{"kind":{"DataType":"String","StringValue":"order"},
                                     "raw":{"DataType":"Binary","BinaryValue":"aGVsbG8="}},
                "Tags":[{"Key":"team","Value":"orders"}]}"#,
        )
        .expect("the document is an object");
        assert_eq!(
            input.required_text("TopicArn").ok(),
            Some("arn:aws:sns:us-east-1:000000000000:orders".to_string()),
            "case \"a text member\": expected the topic ARN"
        );
        assert_eq!(
            input.boolean("ReturnSubscriptionArn").ok().flatten(),
            Some(true),
            "case \"a boolean member\": expected true"
        );
        assert_eq!(
            input.text_list("TagKeys").ok(),
            Some(vec!["team".to_string(), "owner".to_string()]),
            "case \"a list of text\": expected both keys"
        );
        assert_eq!(
            input
                .text_map("Attributes")
                .ok()
                .flatten()
                .and_then(|held| held.get("RawMessageDelivery").cloned()),
            Some("true".to_string()),
            "case \"a map of text\": expected the attribute"
        );
        let attributes = input
            .structure_map("MessageAttributes")
            .expect("case \"a map of structures\": expected the attributes");
        assert_eq!(
            attributes.len(),
            2,
            "case \"a map of structures\": expected both entries, got {}",
            attributes.len()
        );
        let binary = attributes
            .iter()
            .find(|(name, _value)| name == "raw")
            .map(|(_name, value)| value)
            .expect("case \"a map of structures\": expected the binary entry");
        assert_eq!(
            binary.blob("BinaryValue").ok().flatten(),
            Some(b"hello".to_vec()),
            "case \"bytes carried as base64\": expected the decoded bytes"
        );
        let tags = input
            .required_structures("Tags")
            .expect("case \"a list of structures\": expected the tags");
        assert_eq!(
            tags.first().and_then(|tag| tag.text("Key").ok().flatten()),
            Some("team".to_string()),
            "case \"a list of structures\": expected the first tag's key"
        );
        assert!(
            input.text("ReturnSubscriptionArn").is_err(),
            "case \"a boolean read as text\": expected a call that was not made"
        );
        assert!(
            input.boolean("TopicArn").is_err(),
            "case \"text read as a boolean\": expected a call that was not made"
        );
        assert!(
            input.text_list("Attributes").is_err(),
            "case \"a map read as a list\": expected a call that was not made"
        );
        assert!(
            input.required_text("Message").is_err(),
            "case \"a required member the call omits\": expected a call that was not made"
        );
        assert!(
            input.required_structures("Absent").is_err(),
            "case \"a required list the call omits\": expected a call that was not made"
        );
    }

    /// RULE: a member whose shape is an `integer` is sent only with a value that shape can
    /// carry, so a number outside its range stops the call rather than being sent clamped —
    /// a clamped value would make a call asking for something other than what the handler
    /// asked for. The rows differ along where the value sits relative to that range: inside
    /// it, on each bound of the shape, one past each bound, and absent altogether.
    #[test]
    fn a_number_outside_the_integer_shapes_range_stops_the_call() {
        let ceiling = i64::from(i32::MAX);
        let floor = i64::from(i32::MIN);
        // `None` is a call that was not made; `Some(None)` a call carrying no value at all.
        let rows: [(&str, String, Option<Option<i32>>); 6] = [
            ("a value inside the range", "5".to_string(), Some(Some(5))),
            (
                "the largest value the shape carries",
                ceiling.to_string(),
                Some(Some(i32::MAX)),
            ),
            (
                "the smallest value the shape carries",
                floor.to_string(),
                Some(Some(i32::MIN)),
            ),
            ("one past the largest", (ceiling + 1).to_string(), None),
            ("one past the smallest", (floor - 1).to_string(), None),
            ("no value at all", "null".to_string(), Some(None)),
        ];
        for (case, held, expected) in rows {
            let input = Input::read(SERVICE, &format!(r#"{{"MaxItems":{held}}}"#))
                .expect("the row's document is an object");
            let answered = input.integer("MaxItems");
            match expected {
                Some(carried) => assert_eq!(
                    answered.ok(),
                    Some(carried),
                    "case {case:?}: expected the call to carry {carried:?}"
                ),
                None => assert!(
                    answered.is_err(),
                    "case {case:?}: expected a call that was not made, got {:?}",
                    answered.ok()
                ),
            }
        }
    }

    /// RULE: an `integer` member is read in EITHER spelling an SDK takes it in — the number
    /// itself, or the decimal text of it, which is how an SDK models a whole number an
    /// operation binds to its request's query string — so the two spellings carry the same
    /// value into the same call; a value that is no whole number the shape can hold stops the
    /// call whichever spelling it arrived in. The rows differ along the spelling and along the
    /// value: a number and the text of that number, a negative one both ways, the bounds of
    /// the shape as text, text one past a bound, text spelling no number at all, and a
    /// fraction.
    #[test]
    fn an_integer_member_is_read_in_either_spelling() {
        let ceiling = i64::from(i32::MAX);
        let floor = i64::from(i32::MIN);
        // The document's own text for the member, and the value the call then carries —
        // `None` for a call that is not made at all.
        let rows: [(&str, String, Option<i32>); 10] = [
            ("a number", "7".to_string(), Some(7)),
            ("the decimal text of that number", "\"7\"".to_string(), Some(7)),
            ("a negative number", "-3".to_string(), Some(-3)),
            ("the decimal text of a negative number", "\"-3\"".to_string(), Some(-3)),
            ("text spelling zero", "\"0\"".to_string(), Some(0)),
            (
                "text spelling the largest value the shape carries",
                format!("\"{ceiling}\""),
                Some(i32::MAX),
            ),
            (
                "text spelling the smallest value the shape carries",
                format!("\"{floor}\""),
                Some(i32::MIN),
            ),
            (
                "text spelling one past the largest",
                format!("\"{}\"", ceiling + 1),
                None,
            ),
            ("text spelling no number at all", "\"a few\"".to_string(), None),
            ("a fraction", "1.5".to_string(), None),
        ];
        for (case, held, expected) in rows {
            let input = Input::read(SERVICE, &format!(r#"{{"MaxItems":{held}}}"#))
                .expect("the row's document is an object");
            let answered = input.integer("MaxItems");
            match expected {
                Some(carried) => assert_eq!(
                    answered.ok(),
                    Some(Some(carried)),
                    "case {case:?}: expected the call to carry {carried}"
                ),
                None => assert!(
                    answered.is_err(),
                    "case {case:?}: expected a call that was not made, got {:?}",
                    answered.ok()
                ),
            }
        }
    }

    /// RULE: an instant is handed over as the epoch seconds a JSON protocol carries a timestamp
    /// as, FRACTION INCLUDED, so a handler reads back the instant the service reported rather
    /// than one truncated to its whole second.
    ///
    /// The rows differ along the fraction the instant carries: none, a half second, a fraction
    /// stated to the microsecond, one stated FINER than a microsecond (truncated the way a
    /// parser of this wire truncates it, so the two readings of one instant agree), and an
    /// instant before the epoch, whose seconds are negative while its fraction is not.
    #[test]
    fn an_instant_is_carried_as_the_epoch_seconds_its_fraction_included() {
        use ::aws_smithy_types::DateTime;

        let rows: [(&str, DateTime, f64); 5] = [
            ("a whole second", DateTime::from_secs(1_761_000_000), 1_761_000_000.0),
            (
                "half a second",
                DateTime::from_secs_and_nanos(1_761_000_000, 500_000_000),
                1_761_000_000.5,
            ),
            (
                "a fraction stated to the microsecond",
                DateTime::from_secs_and_nanos(1_761_000_000, 389_170_000),
                1_761_000_000.389_17,
            ),
            (
                "a fraction stated finer than a microsecond",
                DateTime::from_secs_and_nanos(1_761_000_000, 389_170_923),
                1_761_000_000.389_17,
            ),
            (
                "an instant before the epoch",
                DateTime::from_secs_and_nanos(-2, 250_000_000),
                -1.75,
            ),
        ];
        for (case, instant, expected) in rows {
            let mut document = Map::new();
            super::insert_timestamp(&mut document, "At", Some(&instant));
            let carried = document
                .get("At")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case:?}: expected the member to be carried"));
            // Compared to the microsecond, which is the resolution this seam carries one at.
            assert!(
                (carried - expected).abs() < 5e-7,
                "case {case:?}: expected the epoch seconds {expected}, got {carried}"
            );
        }
    }
}
