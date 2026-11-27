Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2453612a9812cc17d3b11d7e571f512669259467

# ListWorkspaces refuses a bad nextToken with the synthetic ValidationException instead of this service's modelled one

_Filed: 2026-11-27T16:08:30Z_
_Operation: ListWorkspaces | Class: error-response_

> _Note: this issue was identified against the code base at commit `2453612a9812cc17d3b11d7e571f512669259467`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All four paginated readers of this crate refuse a `nextToken` the operation never issued, but `ListWorkspaces` answers it through the scaffold's SYNTHETIC `smithy.framework#ValidationException` (message only), while `ListVersions`, `ListWorkspaceServiceAccounts` and `ListWorkspaceServiceAccountTokens` answer the identical failure with this service's MODELLED `com.amazonaws.grafana#ValidationException`, carrying `reason` and `fieldList`.

The model marks `reason` `@required` on that shape, so the body `ListWorkspaces` returns for a refused token omits a member the service's own error shape requires, and a client that branches on `reason` / reads `fieldList` gets nothing from this one operation while getting both from its siblings.

The crate's own error-channel documentation names this exact situation and the remedy: `Scaffolded::validation` is to be used only when the model carries no client-input error, and widening an operation that does not list a shape the SERVICE models is what `#[operation_error]`'s `extra_errors` is for. `ListWorkspaces` is precisely the "service models it, this operation does not list it" case, and it took the synthetic path instead.

## Evidence

The handler under review — src/ops/list_workspaces.rs:167-176:
```
        // The token is the id the next page starts at. This operation models no
        // `ValidationException`, so a token that is not a workspace id is
        // refused through the service's own 400 channel.
        if let Some(token) = input.next_token.as_deref() {
            if !crate::state::is_workspace_id(token) {
                return Err(crate::scaffolded::Scaffolded::validation(format!(
                    "nextToken {token:?} is not a token returned by ListWorkspaces"
                )));
            }
        }
```
That variant serialises only a `message` — src/scaffolded.rs:146-150 and :173-178:
```
/// The payload of a `ValidationException`: why the request was refused.
pub struct ValidationException {
    pub message: String,
}
...
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&VALIDATION_EXCEPTION_MESSAGE, self.message.as_str())?;
```

The crate's own instruction for this case — src/scaffolded.rs:215-220:
```
    /// If this service's model declares its own `ValidationException` (or another
    /// client-input error) use THAT: the modeled shape may carry members beyond
    /// `message` — a `reason`, a field list — and a real client reads them. When the
    /// operation already lists it, it is a plain variant on the generated error enum;
    /// `#[operation_error]`'s `extra_errors` is only for widening in a shape the
    /// SERVICE models that this OPERATION does not list.
```

The sibling refusing the SAME failure with the modelled shape — src/ops/list_versions.rs:225-240:
```
                let Some(index) = versions.iter().position(|version| version == token) else {
                    return Err(ListVersionsErrorModeled::ValidationException(
                        ValidationException {
                            message: format!(
                                "nextToken {token:?} is not a token returned by ListVersions"
                            ),
                            reason: "FIELD_VALIDATION_FAILED".to_string(),
                            field_list: Some(vec![ValidationExceptionField {
                                name: "nextToken".to_string(),
```
(the same construction appears in list_workspace_service_accounts.rs:213-232 and list_workspace_service_account_tokens.rs:215-234).

The model's shape requires `reason` — `smithy_query(query="shape", shape="ValidationException")`:
```
"ValidationException": {"shape_id": "com.amazonaws.grafana#ValidationException", "errorKind": "client", "httpError": 400, "members": {"message": {"target": "String", "required": true}, "reason": {"target": "ValidationExceptionReason", "required": true}, "fieldList": {...}}}
```

Test coverage: tests/all/workspace_lifecycle.rs:526-538 asserts only the status and the `__type` for this refusal ("a nextToken that is not a workspace id must be refused" / "a refused nextToken must answer ValidationException"), so nothing pins the member-less body; tests/all/list_versions.rs:349-364 asserts the same two facts for the modelled path, so moving ListWorkspaces onto the modelled shape leaves both tests passing.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. GET /workspaces?nextToken=not-a-token — observe 400 with __type ValidationException and a body carrying ONLY `message`.
3. GET /versions?nextToken=not-a-version — observe 400 ValidationException whose body also carries `reason`: "FIELD_VALIDATION_FAILED" and `fieldList`: [{"name":"nextToken",...}].
4. GET /workspaces/{id}/serviceaccounts?nextToken=not-a-number — same modelled body as step 3, confirming ListWorkspaces is the only reader of the four answering the thinner shape.
