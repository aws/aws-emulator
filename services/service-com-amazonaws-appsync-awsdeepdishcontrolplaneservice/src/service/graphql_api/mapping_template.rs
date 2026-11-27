// SPDX-License-Identifier: CC0-1.0

//! Evaluating a resolver's MAPPING TEMPLATE: the other way a resolver's logic is written.
//!
//! "Mapping templates are written in the Apache Velocity Template Language (VTL)"
//! (`EvaluateMappingTemplate`), and a resolver carries one per direction —
//! `Resolver$requestMappingTemplate` "converts … into a request configuration for the
//! selected data source operation" and `Resolver$responseMappingTemplate` "interpret[s]
//! responses from the data source and map[s] it to the shape of the GraphQL field output
//! type". A resolver defined that way carries no `code`, so running it means EVALUATING
//! those two templates ([`super::execution`] is what dispatches the rendered request to the
//! field's data source and reads the rendered response back).
//!
//! What a template is, and what this evaluates:
//!
//! * TEXT, written out as it stands — which for a request template is the JSON request
//!   object the data source is handed, and for a response template the JSON value the field
//!   resolves to;
//! * REFERENCES (`$context.identity`, `${ctx.args.id}`, `$!{ctx.result}`), which name a
//!   value of the template's own context, walk into it by member and index, and call the
//!   `Map`, `List` and `String` methods of the language on it ([`values`]);
//! * the `$util` HELPERS the resolver reference publishes ([`utilities`]), `$util.toJson`,
//!   the `$util.log` lines a run reports and the `$util.error` that refuses a field among
//!   them;
//! * the DIRECTIVES that make it a template rather than a document: `#set`, `#if` /
//!   `#elseif` / `#else` / `#end`, `#foreach` / `#end` with `$foreach` and `$velocityCount`,
//!   `#break`, `#stop`, the `#return` that ends an evaluation where it stands, the `##` and
//!   `#* … *#` comments and the `#[[ … ]]#` block that is written out unparsed.
//!
//! Every run is bounded in host code the template cannot reach — how long it may run (the
//! budget its caller hands it, [`MAX_RUN`] at most), how many steps it may take
//! ([`MAX_STEPS`]), how large a value it may hold ([`MAX_VALUE_BYTES`]) and how deeply that
//! value may nest ([`MAX_VALUE_DEPTH`]), and how much text it may produce
//! ([`MAX_OUTPUT_BYTES`]) — and so is the PARSE of it, in how long the text may be
//! ([`MAX_TEMPLATE_BYTES`]), how deeply it may nest ([`MAX_NESTING`]) and how far one
//! reference may walk ([`MAX_ACCESSES`]), because the template is caller-supplied and a
//! `#foreach` is a loop. Copying is charged against the step budget by the BYTE
//! ([`BYTES_PER_STEP`]), so a construct that doubles a value runs the budget down in
//! proportion to what it allocates. The CONTEXT is a value, not a handle: a template reads
//! and writes only the JSON it was handed, so nothing of this process is reachable from one,
//! and what it leaves behind (`$ctx.stash`) travels back to the caller as a value too.
//!
//! A template that asks for something this does not implement — a method, a helper — is a
//! FAULT naming it, so the field is reported as one that did not resolve rather than
//! answered with a value nothing computed. So is text that does not PARSE, which is reported
//! as the diagnostic the Velocity parser states for it ([`parse_diagnostic`]).

pub mod utilities;
pub mod values;

/// The `$util.list` and `$util.map` helper families: the collection half of the `$util`
/// library the registry in [`utilities`] dispatches to.
pub mod collections;
/// The `$util.str` and `$util.math` helper families: the scalar half of that library.
pub mod strings_math;
/// The `$util.transform` helper family, which builds the DynamoDB expression document a
/// filter a caller stated comes to.
pub mod transform;

#[cfg(test)]
mod tests;
mod syntax;
pub use syntax::*;

use ::serde_json::{Map as JsonMap, Value as Json};

pub use utilities::Raised;

// The two names ONE value of a resolver context is read under, and how many lines a run may
// log and how long one line may be: stated once for this service
// (`crate::ops::evaluate_code::appsync_js`), so the two languages this sandbox evaluates read
// one context and bound one log list the same way.
use crate::ops::evaluate_code::appsync_js::{
    boundary_at_or_below, ARGUMENTS_MEMBER, ARGUMENTS_SHORT_MEMBER, MAX_LOG_ENTRIES,
    MAX_LOG_ENTRY_BYTES,
};

/// How much TEMPLATE this evaluates: the longest a stored one can be — `MappingTemplate` is
/// `@length 1..65536` (`smithy_query(query="shape",shape="MappingTemplate")`), which is what
/// the write path refuses a longer one against. Stated here too, because what parses
/// caller-supplied text states its own ceiling rather than inheriting one.
const MAX_TEMPLATE_BYTES: usize = 65_536;

/// How many steps one evaluation may take: a reference resolved, an expression evaluated or
/// one pass of a loop body. An emulator resource guard, not an AWS quota.
const MAX_STEPS: usize = 100_000;

/// How long one evaluation may run, whatever it spends its steps on, and the longest budget
/// a caller may give one. The evaluation is synchronous host work, so nothing outside it can
/// stop it: the clock is read as the steps are charged ([`Run::charge`]). The same bound the
/// APPSYNC_JS runs of this service are held to
/// (`crate::ops::evaluate_code::appsync_js`), for the same reason.
const MAX_RUN: ::std::time::Duration = ::std::time::Duration::from_secs(2);

/// How often the clock is read: once every so many steps, so the reading itself costs
/// nothing measurable while still stopping a run whose individual steps are expensive — a
/// copy is charged by the byte ([`BYTES_PER_STEP`]), so a large one spends many steps and
/// meets the clock on the way.
const STEPS_PER_CLOCK_READING: usize = 64;

/// How large a value one evaluation may hold: the ceiling on anything a template builds,
/// stores or grows in place. Memory, not steps: a construct that doubles a value is refused
/// here rather than allowed to outrun the step budget.
const MAX_VALUE_BYTES: usize = 256 * 1024;

/// How deeply a value may nest, for the recursion that walking, weighing, rendering and
/// dropping one takes.
const MAX_VALUE_DEPTH: usize = 32;

/// How many bytes of a value one step pays for: copying is charged against the step budget in
/// proportion to its size, so the work a run can do is bounded in bytes as well as in
/// operations.
const BYTES_PER_STEP: usize = 64;

/// How much text one evaluation may produce, for the same reason as the step budget.
const MAX_OUTPUT_BYTES: usize = 1 << 20;

/// What evaluating one template came to.
///
/// A template that RAISED is one of these too, not a fault: `$util.error` and
/// `$util.unauthorized` are what a template refuses with, and the run that reached one still
/// left a stash behind, still wrote whatever lines it logged and still appended whatever
/// errors it appended before it — so every caller reads what the run left whichever way it
/// ended. Only text this cannot evaluate at all is a [`Fault`].
#[derive(Debug, Clone)]
pub struct Evaluation {
    /// The rendered text — or, for a run an early `#return` ended with a value of its own,
    /// that value as the text it renders to.
    pub text: String,
    /// The context as the template left it, which is what carries the `stash` on to the
    /// next step of the resolution.
    pub context: Json,
    /// The errors `$util.appendError` appended, which do not stop an evaluation.
    pub appended: Vec<Raised>,
    /// The lines `$util.log.info` and `$util.log.error` wrote, in the order the run wrote
    /// them.
    pub logs: Vec<String>,
    /// The error the template RAISED, for a run one ended: `$util.error`, or the refusal
    /// `$util.unauthorized` states. Boxed, because what a raise carries is far larger than
    /// the rest of this answer.
    pub raised: Option<Box<Raised>>,
    /// Whether an early `#return` ended the run rather than the end of the template — a run
    /// that was CUT SHORT, which is reported differently from one that ran through.
    pub returned_early: bool,
}

/// Why an evaluation produced no text.
#[derive(Debug, Clone)]
pub enum Fault {
    /// The template RAISED: `$util.error` or `$util.unauthorized`, which is how a template
    /// refuses the field it is resolving. Boxed, because what a raise carries is far larger
    /// than the rest of this error channel.
    Raised(Box<Raised>),
    /// The template could not be evaluated at all: it does not parse, it asks for something
    /// this does not implement, or it ran past one of the bounds above.
    Fault(String),
}

impl Fault {
    /// What this fault states, as the error of the field that did not resolve.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Raised(raised) => raised.message.clone(),
            Self::Fault(reason) => reason.clone(),
        }
    }
}

/// Evaluate `template` over `context` — the `$context` (`$ctx`) a resolver reads — and
/// answer the text it rendered, the state the run left behind, and the error it raised where
/// it raised one.
///
/// `budget` is how long this run may take, capped at [`MAX_RUN`]: the caller holds the
/// request's own remaining time, so the templates of one request cannot add up past it.
pub fn evaluate(template: &str, context: &Json, budget: ::std::time::Duration) -> Result<Evaluation, Fault> {
    if template.len() > MAX_TEMPLATE_BYTES {
        return Err(Fault::Fault(format!(
            "{TOO_LONG_A_TEMPLATE} {MAX_TEMPLATE_BYTES} bytes."
        )));
    }
    let nodes = parse_with_diagnosis(template)
        .map_err(|failure| Fault::Fault(parse_diagnostic(template, &failure)))?;
    let mut env = JsonMap::new();
    env.insert(CONTEXT_KEY.to_string(), context.clone());
    let mut run = Run {
        env,
        out: String::new(),
        steps: 0,
        started: ::std::time::Instant::now(),
        budget: budget.min(MAX_RUN),
        appended: Vec::new(),
        logs: Vec::new(),
        returned: None,
    };
    // A RAISE is an answer about the template, so the run it ended is reported with
    // everything it left behind; anything else that stopped the run is text this cannot
    // evaluate at all.
    let (raised, flow) = match run.block(&nodes) {
        Ok(flow) => (None, flow),
        Err(Fault::Raised(raised)) => (Some(raised), Flow::Stop),
        Err(fault) => return Err(fault),
    };
    let returned_early = flow == Flow::Return;
    Ok(Evaluation {
        context: run
            .env
            .get(CONTEXT_KEY)
            .cloned()
            .unwrap_or(Json::Object(JsonMap::new())),
        // "#return(data)" ends the evaluation with the value it names as the template's own
        // result; one naming no value ends it with the text rendered up to that point.
        text: run.returned.unwrap_or(run.out),
        appended: run.appended,
        logs: run.logs,
        raised,
        returned_early,
    })
}

/// The one key the context is held under, which both of its names resolve to: a template
/// reads it as `$context` or as `$ctx` ("`$ctx` … an alias of `$context`"), so a `#set` made
/// through one name is read back through the other.
const CONTEXT_KEY: &str = "ctx";

/// The other name the context is read under.
const CONTEXT_ALIAS: &str = "context";

/// The root a `$util` / `$utils` reference names, which is not a value of the context.
const UTILITIES_ROOTS: &[&str] = &["util", "utils"];

/// Which variable a root name reads.
fn root_key(root: &str) -> &str {
    if root == CONTEXT_ALIAS {
        CONTEXT_KEY
    } else {
        root
    }
}

// ---------------------------------------------------------------------------------------
// The template, parsed.
// ---------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------
// The template, evaluated.
// ---------------------------------------------------------------------------------------

/// Where one evaluation reached a loop or the end of the template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Break,
    Stop,
    /// An early `#return` ended the whole evaluation, which is a run CUT SHORT rather than
    /// one that reached the end of its template.
    Return,
}

/// One evaluation: the variables it holds, the text it has produced, and the bounds it is
/// held to.
struct Run {
    env: JsonMap<String, Json>,
    out: String,
    steps: usize,
    /// When this evaluation began, which is what its budget is measured from.
    started: ::std::time::Instant,
    /// How long this evaluation may run, at most [`MAX_RUN`].
    budget: ::std::time::Duration,
    appended: Vec<Raised>,
    /// The lines `$util.log.info` / `$util.log.error` wrote, under [`MAX_LOG_ENTRIES`].
    logs: Vec<String>,
    /// The value an early `#return` named, as the text it renders to, which is the whole
    /// result of a run that reached one.
    returned: Option<String>,
}

impl Run {
    /// Charge one step against this run's budget, and against its clock.
    fn charge(&mut self) -> Result<(), Fault> {
        self.charge_steps(1)
    }

    /// Charge `steps` of them at once — what copying a value costs, by the byte
    /// ([`BYTES_PER_STEP`]).
    fn charge_steps(&mut self, steps: usize) -> Result<(), Fault> {
        let before = self.steps;
        self.steps = self.steps.saturating_add(steps.max(1));
        if self.steps > MAX_STEPS {
            return Err(Fault::Fault(format!("{TOO_MANY_STEPS} {MAX_STEPS}.")));
        }
        // The clock is read when the count crosses a reading point, so a charge of many
        // steps at once meets it too.
        if before / STEPS_PER_CLOCK_READING != self.steps / STEPS_PER_CLOCK_READING
            && self.started.elapsed() > self.budget
        {
            return Err(Fault::Fault(format!(
                "{TOO_LONG} {} ms.",
                self.budget.as_millis()
            )));
        }
        Ok(())
    }

    /// Account for a value this run has built or is about to hold: refuse one past the size
    /// or the nesting a value may have, and charge what copying it cost.
    ///
    /// This is the MEMORY bound beside the step budget: a construct that grows a value — a
    /// list or map built from what a variable already holds, a string added to itself, a
    /// collection added to itself — is refused at [`MAX_VALUE_BYTES`] and pays for every byte
    /// it copied on the way, so it cannot outrun the budget.
    fn account(&mut self, value: &Json) -> Result<(), Fault> {
        let weight = weigh(value, 0).map_err(Fault::Fault)?;
        self.charge_steps(weight / BYTES_PER_STEP)
    }

    /// Write `text` into the rendered output.
    fn write(&mut self, text: &str) -> Result<(), Fault> {
        if self.out.len().saturating_add(text.len()) > MAX_OUTPUT_BYTES {
            return Err(Fault::Fault(format!(
                "{TOO_MUCH_OUTPUT} {MAX_OUTPUT_BYTES} bytes."
            )));
        }
        self.out.push_str(text);
        Ok(())
    }

    /// Evaluate a block of nodes.
    fn block(&mut self, nodes: &[Node]) -> Result<Flow, Fault> {
        for node in nodes {
            self.charge()?;
            match node {
                Node::Text(text) => self.write(text)?,
                Node::Write {
                    reference,
                    quiet,
                    literal,
                } => {
                    let resolved = self.resolve(reference)?;
                    match resolved {
                        // A reference that names nothing writes its own text, which is what
                        // Velocity renders an unresolved one as — unless it was written
                        // quietly (`$!`), which is what quiet means.
                        Resolved::Missing => {
                            if !quiet {
                                let literal = literal.clone();
                                self.write(&literal)?;
                            }
                        }
                        // A call made for its effect rather than for its value writes
                        // nothing at all.
                        Resolved::Nothing => {}
                        Resolved::Value(value) => {
                            let rendered = values::rendered(&value);
                            self.write(&rendered)?;
                        }
                    }
                }
                Node::Set { target, value } => {
                    let value = self.expr(value)?;
                    self.assign(target, value)?;
                }
                Node::If {
                    branches,
                    otherwise,
                } => {
                    let mut taken = false;
                    for (condition, body) in branches {
                        let decided = self.expr(condition)?;
                        if values::truthy(Some(&decided)) {
                            taken = true;
                            match self.block(body)? {
                                Flow::Continue => {}
                                other => return Ok(other),
                            }
                            break;
                        }
                    }
                    if !taken {
                        match self.block(otherwise)? {
                            Flow::Continue => {}
                            other => return Ok(other),
                        }
                    }
                }
                Node::ForEach { name, over, body } => match self.iterate(name, over, body)? {
                    ended @ (Flow::Stop | Flow::Return) => return Ok(ended),
                    _ => continue,
                },
                Node::Break => return Ok(Flow::Break),
                Node::Stop => return Ok(Flow::Stop),
                // `#return(data)` ends the evaluation AT this point and makes the value it
                // names the template's own result; `#return` with no value ends it with what
                // the template has rendered so far. Either way nothing after it runs.
                Node::Return(value) => {
                    if let Some(expression) = value {
                        let produced = self.expr(expression)?;
                        self.returned = Some(values::rendered(&produced));
                    }
                    return Ok(Flow::Return);
                }
            }
        }
        Ok(Flow::Continue)
    }

    /// One `#foreach`: each element of the value, under the loop's own variable and beside
    /// the `$foreach` and `$velocityCount` the body reads.
    fn iterate(&mut self, name: &str, over: &Expr, body: &[Node]) -> Result<Flow, Fault> {
        let elements = match self.expr(over)? {
            Json::Array(list) => list,
            Json::Object(map) => map.values().cloned().collect(),
            Json::Null => Vec::new(),
            single => vec![single],
        };
        let held = self.env.get(name).cloned();
        let held_loop = self.env.get(FOREACH_VARIABLE).cloned();
        let held_count = self.env.get(COUNT_VARIABLE).cloned();
        let mut flow = Flow::Continue;
        for (index, element) in elements.iter().enumerate() {
            self.charge()?;
            self.env.insert(name.to_string(), element.clone());
            let count = index.saturating_add(1);
            self.env.insert(
                FOREACH_VARIABLE.to_string(),
                ::serde_json::json!({
                    "index": index,
                    "count": count,
                    "hasNext": count < elements.len(),
                }),
            );
            self.env
                .insert(COUNT_VARIABLE.to_string(), ::serde_json::json!(count));
            match self.block(body)? {
                Flow::Continue => {}
                Flow::Break => break,
                ended @ (Flow::Stop | Flow::Return) => {
                    flow = ended;
                    break;
                }
            }
        }
        // The loop's own variables are the loop's: what the template held under those names
        // before it stands again after it.
        for (name, held) in [
            (name, held),
            (FOREACH_VARIABLE, held_loop),
            (COUNT_VARIABLE, held_count),
        ] {
            match held {
                Some(value) => self.env.insert(name.to_string(), value),
                None => self.env.remove(name),
            };
        }
        Ok(flow)
    }

    /// What one expression comes to. An absent value is `null`, which is what a condition
    /// and an argument read it as.
    ///
    /// Every value an expression produces is ACCOUNTED for ([`Run::account`]) before it is
    /// answered, so a construct that grows one is refused at the size bound and pays for the
    /// bytes it copied.
    fn expr(&mut self, expression: &Expr) -> Result<Json, Fault> {
        self.charge()?;
        let produced = self.produced(expression)?;
        self.account(&produced)?;
        Ok(produced)
    }

    /// The value one expression produces, before it is accounted for.
    fn produced(&mut self, expression: &Expr) -> Result<Json, Fault> {
        match expression {
            Expr::Literal(value) => Ok(value.clone()),
            Expr::Interpolated(nodes) => {
                // Rendered into THIS run's own output buffer, borrowed for the nested
                // block and handed back: the nested text is charged against the same
                // output budget, and nothing of the environment is copied.
                let held = ::std::mem::take(&mut self.out);
                let nested = self.block(nodes);
                let rendered = ::std::mem::replace(&mut self.out, held);
                nested?;
                Ok(Json::String(rendered))
            }
            Expr::Reference(reference) => Ok(self.resolve(reference)?.value()),
            Expr::List(elements) => {
                let mut values = Vec::with_capacity(elements.len());
                for element in elements {
                    values.push(self.expr(element)?);
                }
                Ok(Json::Array(values))
            }
            Expr::Map(entries) => {
                let mut map = JsonMap::new();
                for (key, value) in entries {
                    let key = self.expr(key)?;
                    let value = self.expr(value)?;
                    map.insert(
                        match key {
                            Json::String(key) => key,
                            other => values::rendered(&other),
                        },
                        value,
                    );
                }
                Ok(Json::Object(map))
            }
            Expr::Not(inner) => {
                let decided = self.expr(inner)?;
                Ok(Json::Bool(!values::truthy(Some(&decided))))
            }
            Expr::Negate(inner) => {
                let value = self.expr(inner)?;
                values::arithmetic('-', &Json::Number(0.into()), &value).map_err(Fault::Fault)
            }
            Expr::Binary(operator, left, right) => self.binary(*operator, left, right),
        }
    }

    /// What one binary expression comes to.
    fn binary(&mut self, operator: Operator, left: &Expr, right: &Expr) -> Result<Json, Fault> {
        // The logical operators decide the left side first and the right side only where it
        // can change the answer, as the language's own short circuit does.
        if matches!(operator, Operator::Or | Operator::And) {
            let left = self.expr(left)?;
            let decided = values::truthy(Some(&left));
            if (operator == Operator::Or) == decided {
                return Ok(Json::Bool(decided));
            }
            let right = self.expr(right)?;
            return Ok(Json::Bool(values::truthy(Some(&right))));
        }
        let left = self.expr(left)?;
        let right = self.expr(right)?;
        let ordered = |decided: ::std::cmp::Ordering, wanted: &[::std::cmp::Ordering]| {
            Json::Bool(wanted.contains(&decided))
        };
        use ::std::cmp::Ordering::{Equal, Greater, Less};
        match operator {
            Operator::Equal => Ok(Json::Bool(values::equal(Some(&left), Some(&right)))),
            Operator::NotEqual => Ok(Json::Bool(!values::equal(Some(&left), Some(&right)))),
            Operator::Less | Operator::LessOrEqual | Operator::Greater
            | Operator::GreaterOrEqual => {
                let decided =
                    values::ordering(Some(&left), Some(&right)).map_err(Fault::Fault)?;
                Ok(match operator {
                    Operator::Less => ordered(decided, &[Less]),
                    Operator::LessOrEqual => ordered(decided, &[Less, Equal]),
                    Operator::Greater => ordered(decided, &[Greater]),
                    _ => ordered(decided, &[Greater, Equal]),
                })
            }
            Operator::Add => values::arithmetic('+', &left, &right).map_err(Fault::Fault),
            Operator::Subtract => values::arithmetic('-', &left, &right).map_err(Fault::Fault),
            Operator::Multiply => values::arithmetic('*', &left, &right).map_err(Fault::Fault),
            Operator::Divide => values::arithmetic('/', &left, &right).map_err(Fault::Fault),
            Operator::Remainder => values::arithmetic('%', &left, &right).map_err(Fault::Fault),
            Operator::Or | Operator::And => Ok(Json::Bool(false)),
        }
    }

    /// What one reference resolves to.
    fn resolve(&mut self, reference: &Reference) -> Result<Resolved, Fault> {
        self.charge()?;
        // Every argument is evaluated BEFORE the value the reference walks into is reached,
        // because an argument may read — and a method may change — that very value.
        let mut args: Vec<Vec<Json>> = Vec::with_capacity(reference.accesses.len());
        for access in &reference.accesses {
            match access {
                Access::Prop(_) => args.push(Vec::new()),
                Access::Index(index) => {
                    let value = self.expr(index)?;
                    args.push(vec![value]);
                }
                Access::Call(_, given) => {
                    let mut row = Vec::with_capacity(given.len());
                    for expression in given {
                        row.push(self.expr(expression)?);
                    }
                    args.push(row);
                }
            }
        }
        if UTILITIES_ROOTS.contains(&reference.root.as_str()) {
            return self.utility(reference, &args);
        }
        let mutating = reference
            .accesses
            .iter()
            .any(|access| matches!(access, Access::Call(..)));
        let key = root_key(&reference.root).to_string();
        let context = key == CONTEXT_KEY;
        let Some(root) = self.env.get_mut(&key) else {
            return Ok(Resolved::Missing);
        };
        // The arguments map is read under either of the two names a resolver context gives
        // it, so a context stating one answers a reference written with the other.
        let aliased = context
            .then(|| argument_alias(root, &reference.accesses))
            .flatten();
        let accesses: &[Access] = aliased.as_deref().unwrap_or(&reference.accesses);
        let walked = walk(root, accesses, &args)
            .map(Resolved::of)
            .map_err(Fault::Fault)?;
        // A method may have CHANGED the value it was called on — `put`, `add`, `addAll` are
        // how a template grows one — so what the variable now holds is accounted for.
        if mutating {
            if let Some(held) = self.env.get(&key).cloned() {
                self.account(&held)?;
            }
        }
        Ok(walked)
    }

    /// What one `$util` reference answers.
    fn utility(&mut self, reference: &Reference, args: &[Vec<Json>]) -> Result<Resolved, Fault> {
        let mut path: Vec<String> = Vec::new();
        let mut called: Option<&[Json]> = None;
        let mut rest: &[Access] = &[];
        let mut rest_args: &[Vec<Json>] = &[];
        for (at, access) in reference.accesses.iter().enumerate() {
            match access {
                Access::Prop(name) => path.push(name.clone()),
                Access::Call(name, _) => {
                    path.push(name.clone());
                    called = args.get(at).map(Vec::as_slice);
                    rest = reference.accesses.get(at.saturating_add(1)..).unwrap_or(&[]);
                    rest_args = args.get(at.saturating_add(1)..).unwrap_or(&[]);
                    break;
                }
                Access::Index(_) => return Err(Fault::Fault(NO_UTIL_INDEX.to_string())),
            }
        }
        // `$util` on its own, or a member of it that is never called, names no helper.
        let Some(given) = called else {
            return Err(Fault::Fault(format!(
                "{NO_SUCH_HELPER}$util{}{}",
                if path.is_empty() { "" } else { "." },
                path.join(".")
            )));
        };
        let answered = utilities::call(&path, given).map_err(Fault::Fault)?;
        match answered {
            utilities::Answer::Value(mut value) => walk(&mut value, rest, rest_args)
                .map(Resolved::of)
                .map_err(Fault::Fault),
            // A helper that answers nothing — and one that answers `null` — writes nothing:
            // the helper WAS called, so its own text is not what the template renders.
            utilities::Answer::Nothing => Ok(Resolved::Nothing),
            utilities::Answer::Raised(raised) => Err(Fault::Raised(Box::new(raised))),
            utilities::Answer::Appended(appended) => {
                self.appended.push(appended);
                Ok(Resolved::Nothing)
            }
            // "A list of logs that were generated by calls to util.log.info and
            // util.log.error in the evaluated code": the line is kept for the answer, and the
            // call itself writes nothing into the rendered text.
            utilities::Answer::Logged(mut line) => {
                if self.logs.len() < MAX_LOG_ENTRIES {
                    if line.len() > MAX_LOG_ENTRY_BYTES {
                        line.truncate(boundary_at_or_below(&line, MAX_LOG_ENTRY_BYTES));
                    }
                    self.logs.push(line);
                }
                Ok(Resolved::Nothing)
            }
        }
    }

    /// `#set`: write `value` where `target` points, creating the maps on the way.
    fn assign(&mut self, target: &Reference, value: Json) -> Result<(), Fault> {
        self.account(&value)?;
        let mut args: Vec<Vec<Json>> = Vec::with_capacity(target.accesses.len());
        for access in &target.accesses {
            match access {
                Access::Index(index) => {
                    let value = self.expr(index)?;
                    args.push(vec![value]);
                }
                _ => args.push(Vec::new()),
            }
        }
        let key = root_key(&target.root).to_string();
        if target.accesses.is_empty() {
            self.env.insert(key, value);
            return Ok(());
        }
        let context = key == CONTEXT_KEY;
        let slot = self
            .env
            .entry(key)
            .or_insert_with(|| Json::Object(JsonMap::new()));
        // A `#set` into the arguments map reaches it under either of its two names too, for
        // the same reason a read of one does.
        let aliased = context
            .then(|| argument_alias(slot, &target.accesses))
            .flatten();
        let accesses: &[Access] = aliased.as_deref().unwrap_or(&target.accesses);
        place(slot, accesses, &args, value).map_err(Fault::Fault)
    }
}

/// `accesses` with the name this context states the ARGUMENTS map under in place of the one
/// the reference wrote, or `None` where no substitution applies.
///
/// `arguments` and `args` name ONE value of a resolver context — "the map that holds all of
/// the contextual information for your resolver invocation" states the field's arguments
/// under both, which is why the context a resolver step is run over carries both names — so a
/// context stating only one of them answers a reference written with the other, and a context
/// stating the name the reference wrote is walked as it stands.
fn argument_alias(root: &Json, accesses: &[Access]) -> Option<Vec<Access>> {
    let Some(Access::Prop(first)) = accesses.first() else {
        return None;
    };
    let Json::Object(members) = root else {
        return None;
    };
    let alias = *ARGUMENT_ALIASES
        .iter()
        .find_map(|(name, alias)| (*name == first.as_str()).then_some(alias))?;
    if members.contains_key(first.as_str()) || !members.contains_key(alias) {
        return None;
    }
    let mut rewritten = accesses.to_vec();
    *rewritten.first_mut()? = Access::Prop(alias.to_string());
    Some(rewritten)
}

/// Each name the arguments map is read under, paired with the other one
/// ([`argument_alias`]).
const ARGUMENT_ALIASES: [(&str, &str); 2] = [
    (ARGUMENTS_MEMBER, ARGUMENTS_SHORT_MEMBER),
    (ARGUMENTS_SHORT_MEMBER, ARGUMENTS_MEMBER),
];

/// The variables a `#foreach` body reads its position from.
const FOREACH_VARIABLE: &str = "foreach";
const COUNT_VARIABLE: &str = "velocityCount";

/// What a reference came to, which is what decides how it is WRITTEN.
#[derive(Debug, Clone)]
enum Resolved {
    /// A value, written as its own text.
    Value(Json),
    /// The reference names nothing: no such variable, no such member, or the member holds
    /// `null`, which Velocity's own renderer treats as not found.
    Missing,
    /// The reference is a call that produced no output of its own.
    Nothing,
}

impl Resolved {
    /// What a walk into a value came to.
    fn of(walked: Option<Json>) -> Self {
        match walked {
            None | Some(Json::Null) => Self::Missing,
            Some(value) => Self::Value(value),
        }
    }

    /// This as the value an expression reads it as: everything that is not a value is
    /// `null`, which is what a condition and an argument take it for.
    fn value(self) -> Json {
        match self {
            Self::Value(value) => value,
            _ => Json::Null,
        }
    }
}

/// Walk `accesses` into `target` and answer what the walk reached, or `None` for a walk that
/// names nothing.
///
/// `target` is borrowed mutably because a method call is allowed to change the value it is
/// called on, which is how a template builds up a map or a list it holds.
fn walk(
    target: &mut Json,
    accesses: &[Access],
    args: &[Vec<Json>],
) -> Result<Option<Json>, String> {
    let Some((first, rest)) = accesses.split_first() else {
        return Ok(Some(target.clone()));
    };
    let (row, rest_args) = split(args);
    match first {
        Access::Prop(name) => {
            if matches!(target, Json::Object(map) if map.contains_key(name)) {
                let Json::Object(map) = target else {
                    return Ok(None);
                };
                let Some(child) = map.get_mut(name) else {
                    return Ok(None);
                };
                return walk(child, rest, rest_args);
            }
            // A property that is no member is the no-argument method of that name, which is
            // how the language reads `$list.size`.
            if values::is_zero_argument_method(target, name) {
                let Some(mut produced) = values::method(target, name, &[])? else {
                    return Ok(None);
                };
                return walk(&mut produced, rest, rest_args);
            }
            Ok(None)
        }
        Access::Index(_) => {
            let index = row.first().cloned().unwrap_or(Json::Null);
            match target {
                Json::Array(list) => {
                    let at = index
                        .as_u64()
                        .and_then(|at| usize::try_from(at).ok())
                        .unwrap_or(usize::MAX);
                    match list.get_mut(at) {
                        Some(child) => walk(child, rest, rest_args),
                        None => Ok(None),
                    }
                }
                Json::Object(map) => {
                    let key = match &index {
                        Json::String(key) => key.clone(),
                        other => values::rendered(other),
                    };
                    match map.get_mut(&key) {
                        Some(child) => walk(child, rest, rest_args),
                        None => Ok(None),
                    }
                }
                _ => Ok(None),
            }
        }
        Access::Call(name, _) => {
            let Some(mut produced) = values::method(target, name, row)? else {
                return Ok(None);
            };
            walk(&mut produced, rest, rest_args)
        }
    }
}

/// Write `value` where `accesses` points within `target`, creating the maps on the way —
/// which is what `#set($ctx.stash.key = …)` does to a stash that holds nothing yet.
fn place(
    target: &mut Json,
    accesses: &[Access],
    args: &[Vec<Json>],
    value: Json,
) -> Result<(), String> {
    let Some((first, rest)) = accesses.split_first() else {
        *target = value;
        return Ok(());
    };
    let (row, rest_args) = split(args);
    if let Access::Call(name, _) = first {
        return Err(format!("{NOT_ASSIGNABLE}{name}"));
    }
    // An index into a list addresses an element that is already there; everything else
    // addresses a member of a map, which is created where it is missing.
    if matches!(first, Access::Index(_)) && target.is_array() {
        let Json::Array(list) = target else {
            return Err(NOT_ASSIGNABLE_ELEMENT.to_string());
        };
        let at = row
            .first()
            .and_then(Json::as_u64)
            .and_then(|at| usize::try_from(at).ok());
        let Some(child) = at.and_then(|at| list.get_mut(at)) else {
            return Err(NOT_ASSIGNABLE_ELEMENT.to_string());
        };
        return place(child, rest, rest_args, value);
    }
    let key = match first {
        Access::Prop(name) => name.clone(),
        _ => match row.first() {
            Some(Json::String(key)) => key.clone(),
            Some(index) => values::rendered(index),
            None => String::new(),
        },
    };
    if !target.is_object() {
        *target = Json::Object(JsonMap::new());
    }
    let Json::Object(map) = target else {
        return Err(NOT_ASSIGNABLE_ELEMENT.to_string());
    };
    let child = map.entry(key).or_insert(Json::Null);
    place(child, rest, rest_args, value)
}

/// The first row of arguments and the rest, for a walk that is about to take one step.
fn split(args: &[Vec<Json>]) -> (&[Json], &[Vec<Json>]) {
    match args.split_first() {
        Some((row, rest)) => (row.as_slice(), rest),
        None => (&[], &[]),
    }
}

/// Roughly how many BYTES a value holds, and whether it is one this evaluation may hold at
/// all.
///
/// `Err` for a value past [`MAX_VALUE_BYTES`] or nested past [`MAX_VALUE_DEPTH`] — the two
/// memory bounds of an evaluation. The walk stops at the first byte past the ceiling, so
/// weighing a refused value costs no more than weighing an admitted one.
///
/// Reached by the field-log trace of an execution too ([`super::execution`]), which bounds
/// what one request RETAINS of the values its fields were resolved with: one service weighs a
/// held value one way.
pub(crate) fn weigh(value: &Json, depth: usize) -> Result<usize, String> {
    if depth > MAX_VALUE_DEPTH {
        return Err(format!("{TOO_DEEP_A_VALUE} {MAX_VALUE_DEPTH}."));
    }
    // One scalar's own footprint, so a list of a thousand nulls weighs something.
    let scalar = ::std::mem::size_of::<Json>();
    let weight = match value {
        Json::Null | Json::Bool(_) | Json::Number(_) => scalar,
        Json::String(text) => scalar.saturating_add(text.len()),
        Json::Array(elements) => {
            let mut weight = scalar;
            for element in elements {
                weight = weight.saturating_add(weigh(element, depth.saturating_add(1))?);
                if weight > MAX_VALUE_BYTES {
                    break;
                }
            }
            weight
        }
        Json::Object(members) => {
            let mut weight = scalar;
            for (name, member) in members {
                weight = weight
                    .saturating_add(name.len())
                    .saturating_add(weigh(member, depth.saturating_add(1))?);
                if weight > MAX_VALUE_BYTES {
                    break;
                }
            }
            weight
        }
    };
    if weight > MAX_VALUE_BYTES {
        return Err(format!("{TOO_LARGE_A_VALUE} {MAX_VALUE_BYTES} bytes."));
    }
    Ok(weight)
}

/// Whether a value this evaluation is ABOUT TO build may be held at all, by the size it will
/// take: the same ceiling [`weigh`] holds a built value to ([`MAX_VALUE_BYTES`]), asked BEFORE
/// the allocation rather than after it.
///
/// A helper whose answer can be far larger than the arguments it was given — a replacement
/// that rewrites every occurrence of a short pattern as a long one — computes the size of its
/// result first and asks this, so the allocation a run is refused for is never made; `weigh`
/// stays the backstop over the value that was built.
pub(crate) fn within_value_bytes(projected: usize) -> Result<(), String> {
    if projected > MAX_VALUE_BYTES {
        return Err(format!("{TOO_LARGE_A_VALUE} {MAX_VALUE_BYTES} bytes."));
    }
    Ok(())
}

/// How a template that does not PARSE is reported: the diagnostic the Velocity parser states
/// for text that stopped being a template — the exception class it raises, the token the parse
/// stopped at, the line and column that token sits at in the template the caller submitted,
/// and the tokens the grammar would have accepted there.
///
/// `failure` is what the parse reported about where it stopped
/// (`::service_common::velocity_template::parse_with_diagnosis`): the index, in characters, and
/// the accepted-token set of that position. A parse that reached the end of the text stopped at
/// the end-of-file token, which is what Velocity names it by, and which the parser states at
/// the last character it read rather than after it.
///
/// The accepted tokens are headed the way the parser heads them — `Was expecting one of:` over
/// a set, `Was expecting:` over a single token — and then listed one four-space-indented
/// `<TOKEN> ...` line each, the last of them followed by that same indent. A parse that stopped
/// against one of this endpoint's own bounds rather than against the grammar has no accepted
/// set, and so is stated as the position alone.
fn parse_diagnostic(template: &str, failure: &ParseFailure) -> String {
    let characters: Vec<char> = template.chars().collect();
    let at = failure.at.min(characters.len());
    let (line, column) = position_of(&characters, at);
    let token = offending_token(&characters, at);
    let mut diagnostic = format!(
        "{PARSE_ERROR}: Encountered \"{token}\" at {TEMPLATE_NAME}[line {line}, column {column}]"
    );
    let accepted = failure.expected.tokens();
    if accepted.is_empty() {
        return diagnostic;
    }
    diagnostic.push('\n');
    diagnostic.push_str(if accepted.len() == 1 {
        WAS_EXPECTING
    } else {
        WAS_EXPECTING_ONE_OF
    });
    for token in accepted {
        diagnostic.push_str(ACCEPTED_INDENT);
        diagnostic.push_str(token);
        diagnostic.push_str(ACCEPTED_MORE);
    }
    diagnostic.push_str(ACCEPTED_INDENT);
    diagnostic
}

/// The token the parse stopped at, as the language's own reader takes it off the text: the
/// run of characters that spell ONE token and nothing of what follows it.
///
/// The reader is the one this service reads a template with
/// (`::service_common::velocity_template`), driven from that one position, so the diagnostic
/// quotes the token the parse itself read rather than a second reading of the text: a
/// reference, a directive, a name, a number or a quoted string is however much of the text
/// that reader takes. Anything else is the operator it spells. Text that ran out stopped at the
/// end-of-input token, which Velocity names [`END_OF_TEMPLATE`], and however long a token is,
/// only [`MAX_TOKEN_CHARACTERS`] of it are quoted.
fn offending_token(characters: &[char], at: usize) -> String {
    let rest = characters.get(at..).unwrap_or_default();
    let Some(&first) = rest.first() else {
        return END_OF_TEMPLATE.to_string();
    };
    let mut reader = Parse::at(characters, at);
    let read = match first {
        '$' => reader.reference().is_some(),
        '#' => reader.directive_name().is_some(),
        '"' | '\'' => reader.quoted(first).is_ok(),
        character if character.is_ascii_digit() => reader.number().is_ok(),
        character if character.is_alphabetic() || character == '_' => {
            reader.identifier().is_some()
        }
        _ => false,
    };
    if read && reader.at > at {
        let end = reader.at.min(at.saturating_add(MAX_TOKEN_CHARACTERS));
        return characters
            .get(at..end)
            .unwrap_or_default()
            .iter()
            .collect();
    }
    // Everything else is the operator the text spells: two characters where the language
    // spells one that way, and the character itself otherwise.
    let pair: String = rest.iter().take(2).collect();
    if TWO_CHARACTER_OPERATORS.contains(&pair.as_str()) {
        return pair;
    }
    first.to_string()
}

/// Where the offending token sits in `characters`, as a parse diagnostic states a position.
///
/// A token of the text is stated where it BEGINS. The end-of-input token has no text of its
/// own: the parser states it at the last character it read, so a template that simply stopped
/// mid-construct is reported at its final character and not at the place after it.
fn position_of(characters: &[char], at: usize) -> (usize, usize) {
    if at < characters.len() {
        return place_of(characters, at);
    }
    match characters.len() {
        // Nothing was read at all, so there is no character the position can point at.
        0 => (1, 0),
        length => place_of(characters, length - 1),
    }
}

/// Which line and column of `characters` the index `at` sits at, both counted from one, as a
/// parse diagnostic states a position.
fn place_of(characters: &[char], at: usize) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;
    for character in characters.iter().take(at) {
        if *character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// The class a Velocity parse failure is raised under, which the diagnostic names, and how it
/// names the end of the template.
const PARSE_ERROR: &str = "org.apache.velocity.exception.ParseErrorException";
/// See [`PARSE_ERROR`].
const END_OF_TEMPLATE: &str = "<EOF>";

/// The name the parser reads a template under, which it states a position INSIDE: a call to
/// this endpoint carries a template and no name for it, so the position reads
/// `velocity[line L, column C]` under the parser's own name for the text it was handed.
const TEMPLATE_NAME: &str = "velocity";

/// How the diagnostic heads the tokens the grammar would have accepted where the parse
/// stopped, and how it writes each of them: one indented line per token, each stated as one way
/// the text could have gone on.
const WAS_EXPECTING_ONE_OF: &str = "Was expecting one of:";
/// See [`WAS_EXPECTING_ONE_OF`]: a position that accepts exactly one token is headed in the
/// singular.
const WAS_EXPECTING: &str = "Was expecting:";
/// See [`WAS_EXPECTING_ONE_OF`].
const ACCEPTED_INDENT: &str = "\n    ";
/// See [`WAS_EXPECTING_ONE_OF`].
const ACCEPTED_MORE: &str = " ...";

/// How much of the token a parse stopped at the diagnostic quotes.
const MAX_TOKEN_CHARACTERS: usize = 32;

/// The operators of the language that are spelled with TWO characters, which is how much of
/// the text one of them takes as the token a parse stopped at ([`offending_token`]). Source:
/// the operator spellings the one reader of this language takes
/// (`::service_common::velocity_template`).
const TWO_CHARACTER_OPERATORS: [&str; 6] = ["==", "!=", "<=", ">=", "&&", "||"];

/// The sentences a template fault states. The ones the SYNTAX states are the language's own
/// ([`::service_common::velocity_template`]); these are what a RUN of a parsed template
/// states, which is this service's own.
const NOT_ASSIGNABLE: &str = "A #set cannot assign to the result of a call: ";
const NOT_ASSIGNABLE_ELEMENT: &str = "A #set cannot assign to that element.";
const NO_UTIL_INDEX: &str = "A $util helper is named, never indexed.";
const NO_SUCH_HELPER: &str = "No such helper: ";
const TOO_LONG_A_TEMPLATE: &str = "This template is longer than this endpoint evaluates:";
const TOO_MANY_STEPS: &str = "This template took more steps than this endpoint evaluates:";
const TOO_LONG: &str = "This template ran for longer than this endpoint evaluates one:";
const TOO_LARGE_A_VALUE: &str = "This template built a value larger than this endpoint holds:";
const TOO_DEEP_A_VALUE: &str =
    "This template built a value nested deeper than this endpoint holds:";
const TOO_MUCH_OUTPUT: &str = "This template produced more text than this endpoint renders:";
