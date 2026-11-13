// SPDX-License-Identifier: CC0-1.0

//! The value of one ARITHMETIC EXPANSION of a bounded shell session — `$((…))`.
//!
//! The grammar is not read here. `brush-parser`, the POSIX/bash grammar this workspace
//! reads caller-supplied shell text with, parses the expansion's text into its own
//! expression tree ([`::brush_parser::arithmetic::parse`]); what this module does is answer
//! what that tree EVALUATES to, which is integer arithmetic over the session's own
//! variables and nothing else. There is no name to resolve outside the session, no command
//! to run and no host to reach, so the whole of an expansion's effect is a number and,
//! where the expression assigns, a variable of the session that was handed in.
//!
//! The session is reached through [`ArithmeticVariables`], so the only state an expansion
//! can read or change is the variables its caller exposes under its own bounds.
//!
//! A form this module does not evaluate — an array element, an expression whose arithmetic
//! overflows, a division by zero — has no value here, and the expansion carrying it is then
//! one its session could not perform, exactly as an unresolvable parameter expansion is.
//! The recursion is bounded by the depth the caller passes, so a deeply nested expression
//! cannot drive this walk past it.

/// The variables an arithmetic expansion reads and assigns: the session's own, under the
/// session's own bounds.
pub trait ArithmeticVariables {
    /// The value held for `name`, as the text the session holds — `None` where it holds
    /// none, which an expansion reads as zero the way a shell does.
    fn arithmetic_value(&self, name: &str) -> Option<String>;

    /// Hold `value` for `name`, answering whether the session took it: a session at its own
    /// bounds does not, and the expansion is then one it could not perform.
    fn set_arithmetic_value(&mut self, name: &str, value: i64) -> bool;
}

/// What `expression` evaluates to in `variables`, or `None` when it has no value here.
///
/// `depth` is how many levels of the expression are left to walk, which is what keeps this
/// recursion in range whatever the expression nests.
pub fn evaluate(
    expression: &::brush_parser::ast::ArithmeticExpr,
    depth: usize,
    variables: &mut dyn ArithmeticVariables,
) -> Option<i64> {
    use ::brush_parser::ast::ArithmeticExpr as Expr;
    let depth = depth.checked_sub(1)?;
    match expression {
        Expr::Literal(value) => Some(*value),
        Expr::Reference(target) => Some(reference_value(target, variables)?),
        Expr::UnaryOp(operator, operand) => {
            let operand = evaluate(operand, depth, variables)?;
            unary(operator, operand)
        }
        Expr::BinaryOp(operator, left, right) => binary(operator, left, right, depth, variables),
        Expr::Conditional(condition, when_true, when_false) => {
            if evaluate(condition, depth, variables)? != 0 {
                evaluate(when_true, depth, variables)
            } else {
                evaluate(when_false, depth, variables)
            }
        }
        Expr::Assignment(target, value) => {
            let ::brush_parser::ast::ArithmeticTarget::Variable(name) = target else {
                return None;
            };
            let value = evaluate(value, depth, variables)?;
            variables.set_arithmetic_value(name, value).then_some(value)
        }
        // Every other form — an array element's target, an assigning operator — is one this
        // module does not evaluate, so the expansion carrying it has no value here.
        _ => None,
    }
}

/// The value a reference to a variable has: what the session holds for it read as an
/// integer, and zero where it holds nothing, which is what an unset parameter is worth in
/// arithmetic. A value that is not an integer at all has no value here.
fn reference_value(
    target: &::brush_parser::ast::ArithmeticTarget,
    variables: &dyn ArithmeticVariables,
) -> Option<i64> {
    let ::brush_parser::ast::ArithmeticTarget::Variable(name) = target else {
        return None;
    };
    let held = variables.arithmetic_value(name).unwrap_or_default();
    let held = held.trim();
    if held.is_empty() {
        return Some(0);
    }
    held.parse::<i64>().ok()
}

/// What one unary operator answers for `operand`, or `None` when the arithmetic overflows.
fn unary(operator: &::brush_parser::ast::UnaryOperator, operand: i64) -> Option<i64> {
    use ::brush_parser::ast::UnaryOperator as Operator;
    match operator {
        Operator::UnaryPlus => Some(operand),
        Operator::UnaryMinus => operand.checked_neg(),
        Operator::BitwiseNot => Some(!operand),
        Operator::LogicalNot => Some(i64::from(operand == 0)),
    }
}

/// What one binary operator answers for its two operands.
///
/// `&&` and `||` evaluate the right operand only when the left does not already settle the
/// answer, the way a shell's own arithmetic does, and both answer 1 or 0. Arithmetic that
/// overflows, and a division or remainder by zero, have no value here.
fn binary(
    operator: &::brush_parser::ast::BinaryOperator,
    left: &::brush_parser::ast::ArithmeticExpr,
    right: &::brush_parser::ast::ArithmeticExpr,
    depth: usize,
    variables: &mut dyn ArithmeticVariables,
) -> Option<i64> {
    use ::brush_parser::ast::BinaryOperator as Operator;
    let first = evaluate(left, depth, variables)?;
    match operator {
        Operator::LogicalAnd => {
            if first == 0 {
                return Some(0);
            }
            return Some(i64::from(evaluate(right, depth, variables)? != 0));
        }
        Operator::LogicalOr => {
            if first != 0 {
                return Some(1);
            }
            return Some(i64::from(evaluate(right, depth, variables)? != 0));
        }
        _ => {}
    }
    let second = evaluate(right, depth, variables)?;
    match operator {
        Operator::Power => u32::try_from(second).ok().and_then(|power| first.checked_pow(power)),
        Operator::Multiply => first.checked_mul(second),
        Operator::Divide => first.checked_div(second),
        Operator::Modulo => first.checked_rem(second),
        Operator::Comma => Some(second),
        Operator::Add => first.checked_add(second),
        Operator::Subtract => first.checked_sub(second),
        Operator::ShiftLeft => u32::try_from(second).ok().and_then(|places| first.checked_shl(places)),
        Operator::ShiftRight => u32::try_from(second)
            .ok()
            .and_then(|places| first.checked_shr(places)),
        Operator::LessThan => Some(i64::from(first < second)),
        Operator::LessThanOrEqualTo => Some(i64::from(first <= second)),
        Operator::GreaterThan => Some(i64::from(first > second)),
        Operator::GreaterThanOrEqualTo => Some(i64::from(first >= second)),
        Operator::Equals => Some(i64::from(first == second)),
        Operator::NotEquals => Some(i64::from(first != second)),
        Operator::BitwiseAnd => Some(first & second),
        Operator::BitwiseXor => Some(first ^ second),
        Operator::BitwiseOr => Some(first | second),
        Operator::LogicalAnd | Operator::LogicalOr => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{evaluate, ArithmeticVariables};

    /// The variables of a session, as this module reaches them.
    #[derive(Debug, Default)]
    struct Held(BTreeMap<String, String>);

    impl ArithmeticVariables for Held {
        fn arithmetic_value(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }

        fn set_arithmetic_value(&mut self, name: &str, value: i64) -> bool {
            self.0.insert(name.to_string(), value.to_string());
            true
        }
    }

    /// What `$((text))` is worth in a session holding `counter=7`.
    fn value_of(text: &str) -> Option<i64> {
        let expression = ::brush_parser::arithmetic::parse(text).ok()?;
        let mut held = Held::default();
        held.0.insert("counter".to_string(), "7".to_string());
        evaluate(&expression, 32, &mut held)
    }

    /// RULE: an arithmetic expansion is integer arithmetic over the session's own variables
    /// — a value, an operator or a comparison, with an unset variable worth zero — and a
    /// form with no value here (overflow, a division by zero) has none rather than a wrong
    /// one.
    #[test]
    fn an_expansion_is_integer_arithmetic_over_the_session() {
        // (case, the expansion's text, its value)
        let rows = [
            ("a literal", "41", Some(41)),
            ("a sum", "1 + 1", Some(2)),
            ("a variable", "counter", Some(7)),
            ("a variable and a literal", "counter + 1", Some(8)),
            ("an unset variable", "missing + 3", Some(3)),
            ("precedence", "2 + 3 * 4", Some(14)),
            ("a grouped expression", "(2 + 3) * 4", Some(20)),
            ("a negation", "-counter", Some(-7)),
            ("a division", "counter / 2", Some(3)),
            ("a remainder", "counter % 4", Some(3)),
            ("a comparison that holds", "counter > 3", Some(1)),
            ("a comparison that does not", "counter > 70", Some(0)),
            ("a logical and", "counter && 0", Some(0)),
            ("a logical or", "0 || counter", Some(1)),
            ("a conditional", "counter > 3 ? 10 : 20", Some(10)),
            ("a shift", "1 << 4", Some(16)),
            ("a bitwise and", "6 & 3", Some(2)),
            ("a division by zero", "counter / 0", None),
            ("an overflow", "9223372036854775807 + 1", None),
        ];
        for (case, text, expected) in rows {
            let value = value_of(text);
            assert_eq!(
                value, expected,
                "case {case:?}: expected {expected:?} for {text:?}, got {value:?}"
            );
        }
    }

    /// RULE: an expansion that ASSIGNS leaves the value on the session's variable, which is
    /// how a program counts something across the lines of one run.
    #[test]
    fn an_assigning_expansion_leaves_the_value_on_the_session() {
        let expression = ::brush_parser::arithmetic::parse("counter = counter + 1")
            .expect("the arithmetic parses");
        let mut held = Held::default();
        held.0.insert("counter".to_string(), "7".to_string());
        let value = evaluate(&expression, 32, &mut held);
        assert_eq!(value, Some(8), "expected the assigned value, got {value:?}");
        assert_eq!(
            held.arithmetic_value("counter").as_deref(),
            Some("8"),
            "the session did not keep the assigned value"
        );
    }

    /// RULE: the walk is bounded by the depth it is given, so a nested expression cannot
    /// drive it past that bound — it has no value instead.
    #[test]
    fn the_walk_is_bounded_by_its_depth() {
        let expression =
            ::brush_parser::arithmetic::parse("1 + (2 + (3 + (4 + 5)))").expect("it parses");
        let mut held = Held::default();
        assert_eq!(
            evaluate(&expression, 32, &mut held),
            Some(15),
            "the expression has a value inside the bound"
        );
        assert_eq!(
            evaluate(&expression, 2, &mut held),
            None,
            "an expression past the bound still had a value"
        );
    }
}
