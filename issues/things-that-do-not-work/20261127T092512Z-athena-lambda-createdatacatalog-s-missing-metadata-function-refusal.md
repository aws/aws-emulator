Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__lambda.hive-catalog-without-metadata-function-rejected
Issue-Key: athena__lambda.hive-catalog-without-metadata-function-rejected|run#17|AssertionError|assert got == want, f"a HIVE catalog without metadata-function was refused with {got!r}"
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# athena__lambda: CreateDataCatalog's missing-metadata-function refusal for a HIVE catalog is emulator-authored, not AWS's message

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataCatalog

## Description
Cause 1 of 1: the HIVE-catalog parameter-validation refusal on CreateDataCatalog carries re-worded message text.

AWS: Smithy: CreateDataCatalog input Name (required), Type (required, DataCatalogType enum), Parameters (ParametersMap), Tags; modeled errors InvalidRequestException (client, HTTP 400) and InternalServerException. Creating a catalog with Type="HIVE" whose Parameters do not contain the metadata-function key is refused with HTTP 400, code InvalidRequestException and the message "Hive catalog parameter metadata-function is required, but not set." — the whole string is fixed text; the only variable element in this message class is the parameter name metadata-function, which is the key the HIVE type requires. Nothing is created: a following GetDataCatalog for that name is itself refused with InvalidRequestException "DataCatalog <name> was not found." (fixed text around the catalog name taken from the request).
Emulator: the correct code and status, but the message text is "Parameters for a HIVE data catalog must contain metadata-function." — an emulator-authored sentence that reverses the subject and drops AWS's "is required, but not set." phrasing.
Why: a client or IaC layer that distinguishes the missing-metadata-function cause from other InvalidRequestException causes by matching AWS's message text cannot classify the refusal, so the operator-facing diagnostic and any automated remediation keyed on it break.
Fix: the CreateDataCatalog handler's HIVE-type parameter validation message.
Traces: athena__lambda.hive-catalog-without-metadata-function-rejected

## Evidence
Expected (code, message) == ('InvalidRequestException', 'Hive catalog parameter metadata-function is required, but not set.'); got ('InvalidRequestException', 'Parameters for a HIVE data catalog must contain metadata-function.'). AssertionError: a HIVE catalog without metadata-function was refused with ('InvalidRequestException', 'Parameters for a HIVE data catalog must contain metadata-function.')

## Steps to Reproduce
Re-run the check in trace_ids. Call athena CreateDataCatalog(Name=<a fresh name>, Type='HIVE', Parameters={'sdk-version': '1.0'}, Tags=[...]) — i.e. a HIVE catalog whose Parameters omit metadata-function — and read Error.Code and Error.Message from the ClientError; then GetDataCatalog(Name=<that name>) to confirm nothing was created.
