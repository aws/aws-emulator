Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-ccp-traj-absent-domain
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-ccp-traj-absent-domain customer-profiles-ccp-traj-search-request-mistakes customer-profiles-ccp-traj-create-input-mistakes customer-profiles-ccp-traj-duplicate-then-dedupe customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling customer-profiles-create-domain-rejects-a-name-outside-the-name-charset
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|SearchProfiles|4xx->4xx|$.Message
Issue-Key: customer-profiles|ListDomains|4xx->4xx|$.Message
Issue-Key: customer-profiles|CreateProfile|4xx->4xx|$.Message
Issue-Key: customer-profiles|CreateProfile|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: customer-profiles|DeleteProfile|4xx->4xx|$.Message
Issue-Key: customer-profiles|CreateDomain|4xx->4xx|$.Message
Issue-Key: customer-profiles|UpdateDomain|4xx->4xx|$.Message
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# customer-profiles 400 refusals carry no Message body: constraint violations serialized without a sentence (7 causes)

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProfile

## Description
Seven net-new wire causes in the customer-profiles crate that share ONE fault: when the emulator refuses a request with HTTP 400 it reaches the right status but serializes NO Message member at all, so the error body carries no sentence. AWS always words a constraint violation, and for member-constraint failures it words it with the fixed AWS validation template '<n> validation error detected: Value \'<value>\' at \'<memberPath>\' failed to satisfy constraint: <constraint sentence>' (the member path is the lowerCamel wire member name, the value comes from the request, and the constraint sentence is derived from the modeled constraint). Every cause below is a mismatch where expected_status == actual_status == 400 and the ONLY diverging key is $.Message (plus the error-type header in Cause 4) - the refusal happens, the explanation does not.

Cause 1 of 7: SearchProfiles does not word the DomainName pattern violation.
AWS: Smithy: SearchProfiles (POST /domains/{DomainName}/profiles/search) binds DomainName as a required @httpLabel targeting `name` (@length 1..64, @pattern ^[a-zA-Z0-9_-]+$) and models BadRequestException with httpError 400. Recorded response: 400 with Message "1 validation error detected: Value '<domainName>' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]+$". Fixed text: "1 validation error detected: Value '" / "' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: "; variable parts: the rejected DomainName from the request path and the pattern taken from the model.
Emulator: status 400, $.Message absent (got None).
Why: an SDK caller reads e.response['Error']['Message'] to learn WHICH member and WHICH constraint failed; an empty message turns every 400 into an opaque refusal, and message-matching error handling that works against AWS sees None.
Fix: the shared constraint-violation message builder used by the customer-profiles input-validation layer (and SearchProfiles' input validation) must render and serialize the Message member for every BadRequestException it raises.
Traces: customer-profiles-ccp-traj-absent-domain#task#4 (cluster also covers customer-profiles-ccp-traj-search-request-mistakes#task#5, #task#6, #task#7).

Cause 2 of 7: ListDomains does not word the MaxResults range violation.
AWS: Smithy: ListDomainsRequest$MaxResults is @httpQuery("max-results") targeting `maxSize100` with @range min 1 max 100; ListDomains models BadRequestException/400. Recorded response: 400 with Message "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1". Fixed text: the validation template plus "Member must have value greater than or equal to "; variable parts: the submitted value, the member path maxResults, and the modeled range minimum.
Emulator: status 400, $.Message absent.
Why: the caller cannot tell a bad page size from any other rejected argument, so pagination bugs are undiagnosable against the emulator.
Fix: ListDomains' query-parameter validation plus the same shared constraint-message builder.
Traces: customer-profiles-ccp-traj-absent-domain#task#5.

Cause 3 of 7: CreateProfile does not word the PartyType enum violation.
AWS: Smithy: CreateProfileRequest$PartyType targets com.amazonaws.customerprofiles#PartyType, an @sensitive (and @deprecated) enum whose values are INDIVIDUAL, BUSINESS, OTHER; CreateProfile models BadRequestException/400. Recorded response: 400 with Message "1 validation error detected: Value at 'partyType' failed to satisfy constraint: Member must satisfy enum value set: [BUSINESS, OTHER, INDIVIDUAL]". Two parts of that are requirements: because the member is @sensitive the offending value is NOT echoed (AWS prints "Value at '<member>'", not "Value '<x>' at '<member>'"), and the set is rendered in AWS's order [BUSINESS, OTHER, INDIVIDUAL]. The whole sentence is fixed text apart from the member path.
Emulator: status 400, $.Message absent entirely.
Why: the enum set in AWS's sentence is how a client discovers the legal values; with no message a caller passing a bad PartyType learns only that something was wrong.
Fix: CreateProfile's input validation and the shared constraint-message builder - suppress the value for @sensitive members and emit AWS's enum ordering.
Traces: customer-profiles-ccp-traj-create-input-mistakes#task#2 (cluster also covers #task#3 and #task#4 of the same fixture).

Cause 4 of 7: a wrongly typed member is answered as an unmodeled ValidationException instead of AWS's deserialization refusal.
AWS: Smithy: CreateProfile's string members (e.g. AccountNumber, FirstName) target sensitiveString1To255; this service models BadRequestException/400 and does NOT model ValidationException. Recorded response: 400 whose body Message is the fixed sentence 'NUMBER_VALUE can not be converted to a String' - AWS's JSON deserialization refusal when a number is supplied for a string member; that sentence is entirely fixed text and interpolates nothing, and the recorded error-type header is a shaped value that is NOT ValidationException.
Emulator: status 400, $.Message absent, and x-amzn-errortype: ValidationException - an error type outside this service's modeled error set.
Why: botocore keys its error classes off x-amzn-errortype, so the caller gets an unmodeled ValidationException that no `except client.exceptions.BadRequestException` handler catches, and no message explaining the type coercion failure.
Fix: the customer-profiles JSON request deserializer and its error mapping - raise the service's modeled refusal with the deserialization sentence rather than inventing a ValidationException error type.
Traces: customer-profiles-ccp-traj-create-input-mistakes#task#5.

Cause 5 of 7: DeleteProfile does not word the ProfileId pattern violation.
AWS: Smithy: DeleteProfileRequest$ProfileId is required, targets `uuid` with @pattern ^[a-f0-9]{32}$; DeleteProfile models BadRequestException/400. Recorded response: 400 with Message "1 validation error detected: Value '<profileId>' at 'profileId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{32}" - note AWS renders the pattern for this member WITHOUT the ^...$ anchors; fixed text is the validation template, variable parts are the submitted id and the member path profileId.
Emulator: status 400, $.Message absent.
Why: a caller that passes a profile name or an ARN where an id belongs gets no indication that the id FORMAT is what failed, and cannot distinguish it from a missing profile.
Fix: DeleteProfile's input validation and the shared constraint-message builder.
Traces: customer-profiles-ccp-traj-duplicate-then-dedupe#task#8.

Cause 6 of 7: CreateDomain does not word the DefaultExpirationDays range violation.
AWS: Smithy: CreateDomainRequest$DefaultExpirationDays is required, targets `expirationDaysInteger` with @range min 1 max 1098; CreateDomain models BadRequestException/400. Recorded response: 400 with Message "1 validation error detected: Value '0' at 'defaultExpirationDays' failed to satisfy constraint: Member must have value greater than or equal to 1"; the ceiling violation is worded with the same template and 'Member must have value less than or equal to 1098'. Variable parts: the submitted value, the member path, the modeled bound.
Emulator: status 400, $.Message absent.
Why: the floor and the ceiling are the two facts a caller needs to pick a retention value; neither is reported.
Fix: CreateDomain's input validation and the shared constraint-message builder.
Traces: customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling#task#1 (cluster also covers #task#2 of the same fixture and customer-profiles-create-domain-rejects-a-name-outside-the-name-charset#task#3).

Cause 7 of 7: UpdateDomain does not word the DefaultExpirationDays range violation either.
AWS: Smithy: UpdateDomainRequest$DefaultExpirationDays targets the same `expirationDaysInteger` (@range 1..1098) and UpdateDomain (PUT /domains/{DomainName}) models BadRequestException/400, so update shares create's floor and ceiling. Recorded response: 400 with Message "1 validation error detected: Value '0' at 'defaultExpirationDays' failed to satisfy constraint: Member must have value greater than or equal to 1".
Emulator: status 400, $.Message absent.
Why: the same opaque refusal on the mutation path, so a client cannot tell a rejected retention change from any other bad update.
Fix: UpdateDomain's input validation and the shared constraint-message builder. (Note: open stub issue 20261029T183204Z-customer-profiles-updatedomain-stub.md exists for UpdateDomain, but the replay shows UpdateDomain SERVING and refusing with 400 - this cause is the missing message on that served refusal, not the stub.)
Traces: customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling#task#4.

Related but not covering: the open behavioral issue 20261102T122410Z-customer-profiles-10-net-new-behavioral-causes-refusal.md describes WORDING defects in messages the emulator does emit (its Cause 3 covers the partyType sentence's content); it does not cover the Message member being absent from these 400 bodies on the wire.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score 49.7%). All seven clusters are verdict=mismatch with expected_status == actual_status == 400; the diverging keys are $.Message (and $headers.x-amzn-errortype in Cause 4).

Cause 1 - fixture customer-profiles-ccp-traj-absent-domain step task#4, SearchProfiles, 400 -> 400:
  $.Message: missing (expected "1 validation error detected: Value 'sim2real.probe.ccp.ad.5a90d3e6' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]+$", got None). affected_count=4.

Cause 2 - fixture customer-profiles-ccp-traj-absent-domain step task#5, ListDomains, 400 -> 400:
  $.Message: missing (expected "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1", got None).

Cause 3 - fixture customer-profiles-ccp-traj-create-input-mistakes step task#2, CreateProfile, 400 -> 400:
  $.Message: missing (expected "1 validation error detected: Value at 'partyType' failed to satisfy constraint: Member must satisfy enum value set: [BUSINESS, OTHER, INDIVIDUAL]", got None). affected_count=3.

Cause 4 - fixture customer-profiles-ccp-traj-create-input-mistakes step task#5, CreateProfile, 400 -> 400:
  $.Message: missing (expected 'NUMBER_VALUE can not be converted to a String', got None)
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException').

Cause 5 - fixture customer-profiles-ccp-traj-duplicate-then-dedupe step task#8, DeleteProfile, 400 -> 400:
  $.Message: missing (expected "1 validation error detected: Value 'not-a-profile-id' at 'profileId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{32}", got None).

Cause 6 - fixture customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling step task#1, CreateDomain, 400 -> 400:
  $.Message: missing (expected "1 validation error detected: Value '0' at 'defaultExpirationDays' failed to satisfy constraint: Member must have value greater than or equal to 1", got None). affected_count=3.

Cause 7 - same fixture step task#4, UpdateDomain, 400 -> 400: identical missing Message for defaultExpirationDays.

In every case the recorded AWS body carries a Message member and the emulator's body has no Message key at all; the values quoted above are one run's fill-ins of the fixed templates described in the description (domain names, profile ids and submitted numbers come from the request).

## Steps to Reproduce
All against the emulator's customer-profiles endpoint; each step is one modeled call.

Cause 1: SearchProfiles(DomainName='sim2real.probe.ccp.ad.5a90d3e6' - dots are outside ^[a-zA-Z0-9_-]+$, KeyName='_account', Values=['1']) -> 400 expected with the domainName pattern sentence; observe a 400 whose body has no Message.
Cause 2: ListDomains(MaxResults=0) -> 400 expected with the maxResults floor sentence; observe no Message.
Cause 3: CreateDomain(D, DefaultExpirationDays=366); CreateProfile(DomainName=D, PartyType=<value outside INDIVIDUAL/BUSINESS/OTHER>) -> 400 expected with the enum-set sentence (value suppressed, set ordered [BUSINESS, OTHER, INDIVIDUAL]); observe no Message.
Cause 4: CreateProfile(DomainName=D, AccountNumber=<a JSON number, not a string>) -> 400 expected with Message 'NUMBER_VALUE can not be converted to a String' and a modeled error type; observe no Message and x-amzn-errortype: ValidationException.
Cause 5: DeleteProfile(DomainName=D, ProfileId='not-a-profile-id') -> 400 expected with the profileId pattern sentence; observe no Message.
Cause 6: CreateDomain(DomainName=D2, DefaultExpirationDays=0) -> 400 expected with the defaultExpirationDays floor sentence; repeat with 1099 for the ceiling sentence; observe no Message in either.
Cause 7: CreateDomain(D3, DefaultExpirationDays=366); UpdateDomain(DomainName=D3, DefaultExpirationDays=0) -> 400 expected with the same floor sentence; observe no Message.

Re-replay fixtures customer-profiles-ccp-traj-absent-domain, customer-profiles-ccp-traj-create-input-mistakes, customer-profiles-ccp-traj-duplicate-then-dedupe and customer-profiles-create-and-update-domain-share-the-expiration-days-floor-and-ceiling to confirm.
