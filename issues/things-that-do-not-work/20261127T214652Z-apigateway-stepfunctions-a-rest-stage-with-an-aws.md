Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__stepfunctions.missing-required-querystring-rejected-starts-no-execution apigateway__stepfunctions.missing-required-run-header-starts-no-execution-rejected apigateway__stepfunctions.signed-caller-account-and-path-ticket-reach-execution-input apigateway__stepfunctions.start-execution-name-from-path-with-tilde-rejected apigateway__stepfunctions.start-execution-reused-name-through-stage-rejected-as-mapped-409
Issue-Key: apigateway__stepfunctions.missing-required-querystring-rejected-starts-no-execution|_wait_for#23|Timeout|time.sleep(every_s)
Issue-Key: apigateway__stepfunctions.missing-required-run-header-starts-no-execution-rejected|_served#32|Timeout|time.sleep(min(2 ** attempt, 20))
Issue-Key: apigateway__stepfunctions.signed-caller-account-and-path-ticket-reach-execution-input|_started#35|Timeout|time.sleep(min(2 ** attempt, 20))
Issue-Key: apigateway__stepfunctions.start-execution-name-from-path-with-tilde-rejected|_settled_start#22|Timeout|time.sleep(pause)
Issue-Key: apigateway__stepfunctions.start-execution-reused-name-through-stage-rejected-as-mapped-409|_send#29|Timeout|time.sleep(min(2 ** attempt, 30))
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# apigateway__stepfunctions: a REST stage with an AWS integration to Step Functions StartExecution/StartSyncExecution never answers, so 5 further request-validation and mapping scenarios reach no verdict

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a deployed REST API stage whose method is an AWS integration to Step Functions (states:StartExecution / StartSyncExecution) produces no graded answer at all — every one of these five scenarios is killed at the 60 s harness cap with no status, body or error recorded.
AWS: recorded behavior only for the integration wiring; each scenario asserts an ordinary API Gateway + Step Functions behaviour that AWS answers promptly:
  - a request missing a method request parameter marked required by the method's request validator is refused 400 by API Gateway and NO execution is started (ListExecutions stays empty) — once for a required querystring and once for a required header;
  - a signed caller's account id and a path-derived ticket are mapped into the execution input, so DescribeExecution's input carries them;
  - a StartExecution name taken from the request path that contains a `~` is refused by Step Functions as an invalid execution name (the name character restriction), and no execution is started;
  - re-using an execution name that already exists is refused ExecutionAlreadyExists, which the method's integration response mapping turns into a 409 to the client.
Emulator: none of the five legs returns — the stage never answers within the budget, so neither the refusals nor the successful mappings can be observed. The emulator is what consumes the budget: it is the stage that must answer.
Why: no API-Gateway-fronted Step Functions API can be exercised at all — a client gets a hang rather than either the execution or the refusal.
Fix: the execute-api AWS-integration path for the Step Functions target (request building, credential assumption and the integration response relay), and the method request validator that must short-circuit before the integration is called.
Traces: apigateway__stepfunctions.missing-required-querystring-rejected-starts-no-execution, apigateway__stepfunctions.missing-required-run-header-starts-no-execution-rejected, apigateway__stepfunctions.signed-caller-account-and-path-ticket-reach-execution-input, apigateway__stepfunctions.start-execution-name-from-path-with-tilde-rejected, apigateway__stepfunctions.start-execution-reused-name-through-stage-rejected-as-mapped-409

The already-open 20261023T210711Z-apigateway-sfn-all-three-api-gateway-to-step owns the three xsvc-apigateway-2-sfn checks of the same path; it does not carry any of the five checks above, so those are reported here.

## Evidence
All five checks: 'Error: Check timed out after 60.0s' — the deployed stage produced no response and no exception, so no status, body or error code was recorded for any leg (the expected answers being, respectively, a 400 from the request validator with ListExecutions empty, the same for the header case, a DescribeExecution input carrying the signed caller's account id and the path ticket, a Step Functions invalid-execution-name refusal for a name containing '~' with no execution started, and a mapped 409 for ExecutionAlreadyExists).

## Steps to Reproduce
1. Create a Step Functions state machine and an IAM role allowing states:StartExecution / states:StartSyncExecution on it.
2. CreateRestApi (REGIONAL); PutMethod POST with a request validator marking a querystring (then a header) as required; PutIntegration type=AWS, integrationHttpMethod=POST, uri=.../states/action/StartExecution, credentials=<role ARN>, with a request template mapping $context.identity.accountId and the path parameter into the execution input and name; PutIntegrationResponse mapping ExecutionAlreadyExists to 409.
3. CreateDeployment + stage; POST through the stage omitting the required parameter, then with it, then with a name containing '~', then re-using an existing execution name; observe that no answer is returned.
