Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 8d1ea8adc587d6ef43eb599364c1f7e78d6ff3ea

# A present-but-empty NextToken is served the first page instead of being refused as a token this operation never issued

_Filed: 2026-11-27T11:42:00Z_
_Operation: DescribeReportDefinitions | Class: pagination_

> _Note: this issue was identified against the code base at commit `8d1ea8adc587d6ef43eb599364c1f7e78d6ff3ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler short-circuits `Some("")` to cursor position 0, so a request carrying `NextToken: ""` is answered HTTP 200 with the first page. That contradicts the rule the same handler states twice: a `NextToken` is honoured ONLY when this store minted it, and a value that is "too short to carry an authenticator" is refused with `invalid_page_token()`. The empty string is a value no mint can produce (`encode_page_token` always renders at least 32 authenticator bytes as hex), and it is the ONLY too-short value the operation accepts: every other short value (the suite's row "hex that is too short to carry an authenticator", token `"ab"`) is refused 400 ValidationException. Routed through `resolve_page_token`, `""` would already be refused — `hex::decode("")` yields zero bytes and `PageTokenKey::cursor_of` returns `None` for a token shorter than `AUTHENTICATOR_BYTES` (services/common/src/page_tokens.rs:82-84) — so the explicit `Some("")` arm is the sole reason it is honoured. The arm carries no comment and no test justifies it.

## Evidence

Stated rule, src/ops/describe_report_definitions.rs:153-155: "/// A `NextToken` is honoured only when THIS store minted it ([`encode_page_token`]): /// the position it names travels inside it behind an authenticator, so a value a /// caller wrote itself names no position and is refused."
Same file, 239-241: "/// Refused with [`invalid_page_token`] when the token is not one this operation issued — /// one that is not hex, one too short to carry an authenticator, one whose authenticator /// does not match the position it travels with ..."
The code that breaks it, src/ops/describe_report_definitions.rs:178-181:
```
let start = match input.next_token.as_deref() {
    None | Some("") => 0,
    Some(token) => resolve_page_token(token, key, &store)?,
};
```
The suite states the rule universally too — tests/all/page_token_issuance.rs:3-6: "a continuation token is honoured only when THIS service issued it, and every other value — however well formed, and whatever it looks like it names — is refused with a 400 `ValidationException`" — and its refused rows include the too-short case at tests/all/page_token_issuance.rs:100: `("hex that is too short to carry an authenticator".to_string(), "ab".to_string())`, yet no row passes the literal empty string, so nothing pins the accepted behaviour.
The boundary validator cannot refuse it either: `NextToken` is length 0..256 with pattern `^[A-Za-z0-9_\.\-=]*$` (smithy_query operation="DescribeReportDefinitions"), both of which the empty string satisfies.

## Steps to Reproduce

1. Create (or seed) any number of report definitions in the emulator.
2. POST / with X-Amz-Target: AWSOrigamiServiceGatewayService.DescribeReportDefinitions and body {"NextToken": ""}.
3. Observe HTTP 200 and the first page of ReportDefinitions.
4. Compare with {"NextToken": "ab"} (also too short to carry an authenticator), which is answered 400 ValidationException "The NextToken in the request is not valid. Pass the NextToken a previous DescribeReportDefinitions response returned." — the two values are classified differently although the handler doc and resolve_page_token's doc put them in the same class.
