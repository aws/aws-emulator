// SPDX-License-Identifier: CC0-1.0

//! The simulator a quantum task's gate-model program is RUN on.
//!
//! A managed simulator device answers a shot-based run with one measurement per shot, so
//! that is what this module produces: the program's circuit is built and evolved by the
//! `qip` state-vector simulator, the crate computes the probability of each outcome of
//! the measured qubits from the final state, and each shot is one draw from that
//! distribution. Nothing is synthesised from the program text: a gate this module cannot
//! build a matrix for stops the run with the reason, and the task that carries it ends in
//! the modelled failure status rather than reporting measurements nothing produced.
//!
//! The run is bounded by the program's own ceilings
//! ([`super::quantum_program::MAX_PROGRAM_QUBITS`] and
//! [`super::quantum_program::MAX_PROGRAM_GATES`]) and by the device's published shot
//! range, which `CreateQuantumTask` holds the request to before anything runs. The work
//! is therefore bounded by a published figure in every direction: the state vector, the
//! gates applied to it, and the draws taken from it.

use std::time::{Duration, Instant};

use qip::prelude::*;
use qip::state_ops::measurement_ops::measure_probs;
use qip::Complex;

use super::quantum_program::{GateCall, GateModelProgram, ProgramRefusal};

/// What a run of a quantum task produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMeasurements {
    /// How many qubits the program that ran DECLARES — the width of the state the run
    /// held, which is also what the device parameters of a gate-model program state.
    pub qubit_count: usize,
    /// The qubits measured, ascending — the columns of each row below.
    pub measured_qubits: Vec<usize>,
    /// One row per shot, each holding one bit per measured qubit.
    pub rows: Vec<Vec<u8>>,
    /// How long the simulation itself took, which is what a simulator reports as the
    /// execution duration of the task it ran.
    pub duration: Duration,
}

/// Lower the program the `action` document carries and run it, on a thread whose stack
/// is sized for the depth the lowering admits.
///
/// This is the ONE entry point a handler drives a quantum task's program through, and it
/// is where the stack the parse runs on is decided. `oq3_syntax`'s parser descends per
/// nested construct and the tree it builds is dropped the same way, and every construct
/// costs at least one byte of the program text, so the LENGTH bound the lowering takes
/// ([`super::quantum_program::MAX_PROGRAM_SOURCE_BYTES`], 8 KiB) is what bounds that
/// descent: at most 8_192 nested constructs, and this stack leaves 4 KiB for each of
/// them — room for several frames per construct at any size a recursive-descent parser
/// frame takes. The simulator's own work is iterative.
///
/// This blocks: call it from a blocking thread, never from the runtime that serves
/// requests.
pub fn execute(action: &str, shots: i64) -> Result<TaskMeasurements, ProgramRefusal> {
    let action = action.to_string();
    let worker = ::std::thread::Builder::new()
        .name("braket-quantum-task".to_string())
        .stack_size(PROGRAM_STACK_BYTES)
        .spawn(move || {
            let program = super::quantum_program::gate_model_program(&action)?;
            run(&program, shots)
        });
    let worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            ::tracing::warn!(
                error = %error,
                "a quantum task's program could not be given a thread to run on"
            );
            return Err(ProgramRefusal::new(
                "The quantum task's program could not be started, so no result was \
                 produced for it."
                    .to_string(),
            ));
        }
    };
    match worker.join() {
        Ok(outcome) => outcome,
        Err(_) => Err(ProgramRefusal::new(
            "The quantum task's program did not run to completion, so no result was \
             produced for it."
                .to_string(),
        )),
    }
}

/// The stack the lowering and the run are given.
///
/// The program text is bounded at [`super::quantum_program::MAX_PROGRAM_SOURCE_BYTES`]
/// (8 KiB) and a parser descends at most once per byte of it, so this leaves 4 KiB of
/// stack per possible nested construct — several frames each, whatever a frame of
/// `oq3_syntax`'s parser costs, and the same headroom for the recursive drop of the tree
/// it builds. It is virtual address space: only the pages a run actually touches are
/// committed, and the thread lives for one quantum task's lowering and run.
pub const PROGRAM_STACK_BYTES: usize = 32 << 20;

/// Run `program` for `shots` shots, or report why it could not be run.
///
/// Every figure the run is driven by is bounded HERE as well as where it came from, so
/// the runner holds its own bounds: the qubits it builds a state vector for, the work the
/// gates cost over that state ([`MAX_AMPLITUDE_UPDATES`], the PRODUCT of the two), the
/// qubits a measurement may name, and the shots drawn from the result ([`MAX_SHOTS`],
/// which the create has already held to the device's published `shotsRange`).
///
/// This is CPU-bound and synchronous: a caller drives it from a blocking thread rather
/// than from the async runtime that serves requests.
pub fn run(program: &GateModelProgram, shots: i64) -> Result<TaskMeasurements, ProgramRefusal> {
    let started = Instant::now();
    if program.qubit_count == 0 {
        return Err(ProgramRefusal::new(
            "The program declares no qubits, so there is nothing to run.".to_string(),
        ));
    }
    if shots <= 0 {
        return Err(ProgramRefusal::new(
            "The quantum task asks for no shots, so no measurement was taken.".to_string(),
        ));
    }
    // The rows a run produces are one per shot, so the shot count is what bounds the
    // memory the measurements take. The create has already held it to the device's
    // published range; it is re-checked here so the bound belongs to the runner too.
    if shots > MAX_SHOTS {
        return Err(ProgramRefusal::new(format!(
            "The quantum task asks for {shots} shots, more than the {MAX_SHOTS} this \
             emulator's simulator draws for one quantum task."
        )));
    }
    // The state vector holds one amplitude per basis state, so the qubit ceiling is what
    // bounds the memory a run needs. The lowering holds a program to it as it reads the
    // declarations; it is re-checked here so the bound belongs to the runner too.
    if program.qubit_count > super::quantum_program::MAX_PROGRAM_QUBITS {
        return Err(ProgramRefusal::new(format!(
            "The program declares more than the {} qubits this emulator's simulator holds \
             a state vector for.",
            super::quantum_program::MAX_PROGRAM_QUBITS
        )));
    }
    // One gate touches every amplitude of the state, so the work a run costs is the gate
    // count times the size of the state: it is the PRODUCT that is bounded, not either
    // factor on its own.
    let amplitudes = 1_u64 << program.qubit_count;
    let updates = amplitudes.saturating_mul(program.gates.len() as u64);
    if updates > MAX_AMPLITUDE_UPDATES {
        return Err(ProgramRefusal::new(format!(
            "The program applies {} gates to {} qubits, which is more work than the \
             {MAX_AMPLITUDE_UPDATES} amplitude updates this emulator's simulator runs for \
             one quantum task.",
            program.gates.len(),
            program.qubit_count
        )));
    }
    // A measurement names a qubit of the program, so an index outside it is one no state
    // this run holds has a column for.
    if let Some(outside) = program
        .measured_qubits
        .iter()
        .find(|qubit| **qubit >= program.qubit_count)
    {
        return Err(ProgramRefusal::new(format!(
            "The program measures qubit {outside}, which is outside the {} it declares.",
            program.qubit_count
        )));
    }
    let probabilities = outcome_probabilities(program)?;
    let draws = usize::try_from(shots).map_err(|_| {
        ProgramRefusal::new(format!("The shot count {shots} is not one a run can take."))
    })?;
    let rows = sample(&probabilities, program.measured_qubits.len(), draws);
    Ok(TaskMeasurements {
        qubit_count: program.qubit_count,
        measured_qubits: program.measured_qubits.clone(),
        rows,
        duration: started.elapsed(),
    })
}

/// The most amplitude updates one quantum task's run may cost: the gate count times the
/// size of the state vector.
///
/// A state-vector simulator applies each gate across the whole state, so this is what
/// bounds the CPU one accepted task can consume however the program divides that work
/// between qubits and gates. It is the emulator's own figure, chosen so a run stays well
/// inside a second on a developer's machine; a program past it is reported as not
/// executed, with the figures that exceeded it.
pub const MAX_AMPLITUDE_UPDATES: u64 = 1 << 24;

/// The most shots one quantum task's run may draw.
///
/// One shot is one row of the result, so this is what bounds the measurements a run
/// holds and the document written from them. It is the highest `shotsRange` maximum any
/// device in this service's inventory publishes, so it refuses nothing the create
/// accepts and leaves the runner bounded on its own.
pub const MAX_SHOTS: i64 = 100_000;

/// The probability of each outcome of the program's measured qubits, computed by the
/// simulator from the state its circuit evolves to.
///
/// Entry `m` is the probability of the outcome whose bit `i` is the value of
/// `program.measured_qubits[i]`, which is the order `qip`'s own measurement op reports
/// (`measure_prob`: "`measured` gives the bits (as a usize) which has been measured from
/// the qubits at `indices` in the order supplied by `indices`").
fn outcome_probabilities(program: &GateModelProgram) -> Result<Vec<f64>, ProgramRefusal> {
    let qubit_count = program.qubit_count;
    let mut builder = LocalBuilder::<f64>::default();
    let mut qubits: Vec<Option<_>> = (0..qubit_count).map(|_| Some(builder.qubit())).collect();
    for gate in &program.gates {
        let matrix = gate_matrix(gate)?;
        let mut operands = Vec::with_capacity(gate.targets.len());
        for target in &gate.targets {
            let register = qubits
                .get_mut(*target)
                .and_then(Option::take)
                .ok_or_else(|| {
                    ProgramRefusal::new(format!(
                        "The gate {} names qubit {target}, which the program does not \
                         declare or which another operand of the same gate already names.",
                        gate.name
                    ))
                })?;
            operands.push(register);
        }
        let merged = builder.merge_registers(operands).ok_or_else(|| {
            ProgramRefusal::new(format!(
                "The gate {} names no qubit to act on.",
                gate.name
            ))
        })?;
        let applied = builder.apply_vec_matrix(merged, matrix).map_err(|error| {
            ProgramRefusal::new(format!(
                "The gate {} could not be applied: {error}.",
                gate.name
            ))
        })?;
        for single in builder.split_all_register(applied) {
            // A register split to single qubits carries exactly one absolute index, which
            // is the slot it goes back into. Nothing is indexed blindly: a register this
            // does not place is one a later gate reports as unavailable rather than one
            // that panics here.
            if let Some(index) = single.indices().first().copied() {
                if let Some(slot) = qubits.get_mut(index) {
                    *slot = Some(single);
                }
            }
        }
    }
    let (state, _) = builder.calculate_state();
    let probabilities = measure_probs(
        qubit_count,
        &program.measured_qubits,
        &state,
        None,
    );
    // A distribution nothing can be drawn from is not one to sample: the run produced no
    // state to measure, and saying so is the honest answer.
    let total: f64 = probabilities.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return Err(ProgramRefusal::new(
            "The program's circuit left no state to measure, so no shot could be drawn \
             from it."
                .to_string(),
        ));
    }
    Ok(probabilities)
}

/// Draw `draws` shots from `probabilities`, each row holding one bit per measured qubit.
///
/// Every shot is an independent draw from the distribution the simulator computed, which
/// is what a shot-based run of a circuit is: the cumulative distribution is built once
/// and each draw is one search in it, so a run of many shots costs the state vector once.
fn sample(probabilities: &[f64], measured_qubits: usize, draws: usize) -> Vec<Vec<u8>> {
    let mut cumulative = Vec::with_capacity(probabilities.len());
    let mut running = 0.0;
    for probability in probabilities {
        running += probability.max(0.0);
        cumulative.push(running);
    }
    let total = running;
    let last = cumulative.len().saturating_sub(1);
    (0..draws)
        .map(|_| {
            let point = qip::rand::random::<f64>() * total;
            let outcome = cumulative.partition_point(|edge| *edge < point).min(last);
            // The outcome's bit `i` is the value of the `i`th measured qubit, which is
            // how the simulator's own measurement op reports it; the value is shifted
            // down one bit per column rather than indexed by a shift count.
            let mut remaining = outcome;
            (0..measured_qubits)
                .map(|_| {
                    let bit = u8::from(remaining & 1 == 1);
                    remaining >>= 1;
                    bit
                })
                .collect()
        })
        .collect()
}

/// `i`, as a complex number.
fn i() -> Complex<f64> {
    Complex::new(0.0, 1.0)
}

/// The complex number of modulus one at angle `theta` — the phase a rotation applies.
fn phase(theta: f64) -> Complex<f64> {
    Complex::from_polar(1.0, theta)
}

/// A real number, as a complex number.
fn re(value: f64) -> Complex<f64> {
    Complex::new(value, 0.0)
}

/// The two-qubit matrix that applies the single-qubit matrix `target` when the FIRST
/// qubit is `|1>`, which is the controlled form of a gate: `[[I, 0], [0, U]]`, with the
/// control as the high bit of the index, as the simulator's own `make_cnot` lays a
/// two-qubit matrix out.
fn controlled(target: [Complex<f64>; 4]) -> Vec<Complex<f64>> {
    let zero = re(0.0);
    let one = re(1.0);
    vec![
        one, zero, zero, zero, //
        zero, one, zero, zero, //
        zero, zero, target[0], target[1], //
        zero, zero, target[2], target[3], //
    ]
}

/// The matrix with `diagonal` on its diagonal and zeroes everywhere else.
fn diagonal(diagonal: &[Complex<f64>]) -> Vec<Complex<f64>> {
    let size = diagonal.len();
    let mut matrix = vec![re(0.0); size * size];
    for (index, value) in diagonal.iter().enumerate() {
        matrix[index * size + index] = *value;
    }
    matrix
}

/// One gate's matrix: how many qubits it acts on, and its rows.
struct GateMatrix {
    qubits: usize,
    data: Vec<Complex<f64>>,
}

/// The matrix of the gate `gate` names, with its parameters applied.
///
/// The names are the gate-model operations this service's device inventory publishes
/// (`super::GATE_MODEL_OPERATIONS`), the gates its devices run natively (`prx`, `cv`),
/// and the aliases the OpenQASM standard gate library gives them (`cx` for `cnot`, `sdg`
/// for `si`, `p` for `phaseshift`, `u` for the general single-qubit unitary, ...), so a
/// program written against any of those vocabularies runs. A name outside them, or a
/// parameter count the gate does not take, stops the run with the reason.
fn gate_matrix(gate: &GateCall) -> Result<Vec<Complex<f64>>, ProgramRefusal> {
    let matrix = matrix_of(&gate.name, &gate.parameters).ok_or_else(|| {
        ProgramRefusal::new(format!(
            "This emulator's simulator does not apply the gate {} with {} parameter(s).",
            gate.name,
            gate.parameters.len()
        ))
    })?;
    if matrix.qubits != gate.targets.len() {
        return Err(ProgramRefusal::new(format!(
            "The gate {} acts on {} qubit(s), but the program named {}.",
            gate.name,
            matrix.qubits,
            gate.targets.len()
        )));
    }
    Ok(matrix.data)
}

/// The matrix of the gate `name` takes with `parameters`, or `None` when this simulator
/// applies no such gate.
#[allow(clippy::too_many_lines)]
fn matrix_of(name: &str, parameters: &[f64]) -> Option<GateMatrix> {
    let zero = re(0.0);
    let one = re(1.0);
    let half_root = re(std::f64::consts::FRAC_1_SQRT_2);
    let single = |data: [Complex<f64>; 4]| {
        Some(GateMatrix {
            qubits: 1,
            data: data.to_vec(),
        })
    };
    let pair = |data: Vec<Complex<f64>>| Some(GateMatrix { qubits: 2, data });
    let angle = |index: usize| parameters.get(index).copied();
    match (name, parameters.len()) {
        // --- single-qubit gates taking no parameter ---
        ("i" | "id", 0) => single([one, zero, zero, one]),
        ("x", 0) => single([zero, one, one, zero]),
        ("y", 0) => single([zero, -i(), i(), zero]),
        ("z", 0) => single([one, zero, zero, -one]),
        ("h", 0) => single([half_root, half_root, half_root, -half_root]),
        ("s", 0) => single([one, zero, zero, i()]),
        ("si" | "sdg", 0) => single([one, zero, zero, -i()]),
        ("t", 0) => single([one, zero, zero, phase(std::f64::consts::FRAC_PI_4)]),
        ("ti" | "tdg", 0) => single([one, zero, zero, phase(-std::f64::consts::FRAC_PI_4)]),
        // V is the square root of X, and Vi its inverse.
        ("v" | "sx", 0) => single([
            Complex::new(0.5, 0.5),
            Complex::new(0.5, -0.5),
            Complex::new(0.5, -0.5),
            Complex::new(0.5, 0.5),
        ]),
        ("vi" | "sxdg", 0) => single([
            Complex::new(0.5, -0.5),
            Complex::new(0.5, 0.5),
            Complex::new(0.5, 0.5),
            Complex::new(0.5, -0.5),
        ]),
        // --- single-qubit rotations ---
        ("rx", 1) => {
            let theta = angle(0)?;
            let (sin, cos) = (theta / 2.0).sin_cos();
            single([re(cos), -i() * sin, -i() * sin, re(cos)])
        }
        ("ry", 1) => {
            let theta = angle(0)?;
            let (sin, cos) = (theta / 2.0).sin_cos();
            single([re(cos), re(-sin), re(sin), re(cos)])
        }
        ("rz", 1) => {
            let theta = angle(0)?;
            single([phase(-theta / 2.0), zero, zero, phase(theta / 2.0)])
        }
        ("phaseshift" | "p" | "u1", 1) => {
            single([one, zero, zero, phase(angle(0)?)])
        }
        // The IonQ native single-qubit gates, as Amazon Braket publishes them.
        ("gpi", 1) => {
            let theta = angle(0)?;
            single([zero, phase(-theta), phase(theta), zero])
        }
        ("gpi2", 1) => {
            let theta = angle(0)?;
            single([
                half_root,
                half_root * -i() * phase(-theta),
                half_root * -i() * phase(theta),
                half_root,
            ])
        }
        // A phased Rx: Rx(theta) conjugated by the phase phi.
        ("prx", 2) => {
            let (theta, phi) = (angle(0)?, angle(1)?);
            let (sin, cos) = (theta / 2.0).sin_cos();
            single([
                re(cos),
                -i() * phase(-phi) * sin,
                -i() * phase(phi) * sin,
                re(cos),
            ])
        }
        // The general single-qubit unitary of OpenQASM.
        ("u" | "u3", 3) => {
            let (theta, phi, lambda) = (angle(0)?, angle(1)?, angle(2)?);
            let (sin, cos) = (theta / 2.0).sin_cos();
            single([
                re(cos),
                -phase(lambda) * sin,
                phase(phi) * sin,
                phase(phi + lambda) * cos,
            ])
        }
        // --- two-qubit gates, control (or first operand) as the high index ---
        ("cnot" | "cx", 0) => pair(controlled([zero, one, one, zero])),
        ("cy", 0) => pair(controlled([zero, -i(), i(), zero])),
        ("cz", 0) => pair(controlled([one, zero, zero, -one])),
        ("cv", 0) => pair(controlled([
            Complex::new(0.5, 0.5),
            Complex::new(0.5, -0.5),
            Complex::new(0.5, -0.5),
            Complex::new(0.5, 0.5),
        ])),
        ("cphaseshift" | "cp" | "cu1", 1) => {
            pair(controlled([one, zero, zero, phase(angle(0)?)]))
        }
        ("cphaseshift00", 1) => pair(diagonal(&[phase(angle(0)?), one, one, one])),
        ("cphaseshift01", 1) => pair(diagonal(&[one, phase(angle(0)?), one, one])),
        ("cphaseshift10", 1) => pair(diagonal(&[one, one, phase(angle(0)?), one])),
        ("crx", 1) => {
            let theta = angle(0)?;
            let (sin, cos) = (theta / 2.0).sin_cos();
            pair(controlled([re(cos), -i() * sin, -i() * sin, re(cos)]))
        }
        ("cry", 1) => {
            let theta = angle(0)?;
            let (sin, cos) = (theta / 2.0).sin_cos();
            pair(controlled([re(cos), re(-sin), re(sin), re(cos)]))
        }
        ("crz", 1) => {
            let theta = angle(0)?;
            pair(controlled([
                phase(-theta / 2.0),
                zero,
                zero,
                phase(theta / 2.0),
            ]))
        }
        ("swap", 0) => pair(vec![
            one, zero, zero, zero, //
            zero, zero, one, zero, //
            zero, one, zero, zero, //
            zero, zero, zero, one,
        ]),
        ("iswap", 0) => pair(vec![
            one, zero, zero, zero, //
            zero, zero, i(), zero, //
            zero, i(), zero, zero, //
            zero, zero, zero, one,
        ]),
        ("pswap", 1) => {
            let swapped = phase(angle(0)?);
            pair(vec![
                one, zero, zero, zero, //
                zero, zero, swapped, zero, //
                zero, swapped, zero, zero, //
                zero, zero, zero, one,
            ])
        }
        ("xx", 1) => {
            let theta = angle(0)?;
            let (sin, cos) = (theta / 2.0).sin_cos();
            let (c, s) = (re(cos), -i() * sin);
            pair(vec![
                c, zero, zero, s, //
                zero, c, s, zero, //
                zero, s, c, zero, //
                s, zero, zero, c,
            ])
        }
        ("yy", 1) => {
            let theta = angle(0)?;
            let (sin, cos) = (theta / 2.0).sin_cos();
            let (c, s) = (re(cos), -i() * sin);
            pair(vec![
                c, zero, zero, -s, //
                zero, c, s, zero, //
                zero, s, c, zero, //
                -s, zero, zero, c,
            ])
        }
        ("zz", 1) => {
            let theta = angle(0)?;
            pair(diagonal(&[
                phase(-theta / 2.0),
                phase(theta / 2.0),
                phase(theta / 2.0),
                phase(-theta / 2.0),
            ]))
        }
        ("xy", 1) => {
            let theta = angle(0)?;
            let (sin, cos) = (theta / 2.0).sin_cos();
            let (c, s) = (re(cos), i() * sin);
            pair(vec![
                one, zero, zero, zero, //
                zero, c, s, zero, //
                zero, s, c, zero, //
                zero, zero, zero, one,
            ])
        }
        // --- three-qubit gates, controls as the high indices ---
        ("ccnot" | "ccx" | "toffoli", 0) => {
            let mut data = diagonal(&[one; 8]);
            // The |11> block of the controls acts as X on the target.
            data[6 * 8 + 6] = zero;
            data[7 * 8 + 7] = zero;
            data[6 * 8 + 7] = one;
            data[7 * 8 + 6] = one;
            Some(GateMatrix { qubits: 3, data })
        }
        ("cswap" | "fredkin", 0) => {
            let mut data = diagonal(&[one; 8]);
            // With the control set, the two targets are swapped.
            data[5 * 8 + 5] = zero;
            data[6 * 8 + 6] = zero;
            data[5 * 8 + 6] = one;
            data[6 * 8 + 5] = one;
            Some(GateMatrix { qubits: 3, data })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::quantum_program::gate_model_program;

    /// The program of an OpenQASM source, as the action document carries it.
    fn program(source: &str) -> GateModelProgram {
        let action = serde_json::json!({
            "braketSchemaHeader": { "name": crate::state::ACTION_OPENQASM, "version": "1" },
            "source": source,
        })
        .to_string();
        gate_model_program(&action).expect("the program lowers")
    }

    /// A circuit whose outcome is certain reports exactly that outcome, on every shot —
    /// which is what pins both the gate matrices and the order of the measured columns.
    #[test]
    fn a_deterministic_circuit_reports_its_one_outcome_on_every_shot() {
        // (case, source, the row every shot must report)
        let cases: Vec<(&str, &str, Vec<u8>)> = vec![
            ("no gate at all", "OPENQASM 3.0; qubit[2] q;", vec![0, 0]),
            (
                "x on the first qubit only",
                "OPENQASM 3.0; qubit[2] q; x q[0];",
                vec![1, 0],
            ),
            (
                "x on the second qubit only",
                "OPENQASM 3.0; qubit[2] q; x q[1];",
                vec![0, 1],
            ),
            (
                "cnot with the control set",
                "OPENQASM 3.0; qubit[2] q; x q[0]; cnot q[0], q[1];",
                vec![1, 1],
            ),
            (
                "cnot with the control clear",
                "OPENQASM 3.0; qubit[2] q; cnot q[0], q[1];",
                vec![0, 0],
            ),
            (
                "swap moves the excitation",
                "OPENQASM 3.0; qubit[2] q; x q[0]; swap q[0], q[1];",
                vec![0, 1],
            ),
            (
                "ccnot with both controls set",
                "OPENQASM 3.0; qubit[3] q; x q[0]; x q[1]; ccnot q[0], q[1], q[2];",
                vec![1, 1, 1],
            ),
            (
                "ccnot with one control clear",
                "OPENQASM 3.0; qubit[3] q; x q[0]; ccnot q[0], q[1], q[2];",
                vec![1, 0, 0],
            ),
            (
                "cswap with the control set",
                "OPENQASM 3.0; qubit[3] q; x q[0]; x q[1]; cswap q[0], q[1], q[2];",
                vec![1, 0, 1],
            ),
            (
                "rx by pi is an x",
                "OPENQASM 3.0; qubit[1] q; rx(pi) q[0];",
                vec![1],
            ),
            (
                "ry by pi is an x up to phase",
                "OPENQASM 3.0; qubit[1] q; ry(pi) q[0];",
                vec![1],
            ),
            (
                "two h gates cancel",
                "OPENQASM 3.0; qubit[1] q; h q[0]; h q[0];",
                vec![0],
            ),
            (
                "v twice is an x",
                "OPENQASM 3.0; qubit[1] q; v q[0]; v q[0];",
                vec![1],
            ),
            (
                "a phase gate leaves the measured value alone",
                "OPENQASM 3.0; qubit[1] q; x q[0]; s q[0]; t q[0]; z q[0];",
                vec![1],
            ),
            (
                "only the measured qubit is a column",
                "OPENQASM 3.0; bit b; qubit[2] q; x q[1]; b = measure q[1];",
                vec![1],
            ),
        ];
        for (case, source, expected) in cases {
            let measurements = run(&program(source), 8).expect("the circuit runs");
            assert_eq!(
                measurements.rows.len(),
                8,
                "case {case}: expected one row per shot, got {:?}",
                measurements.rows
            );
            for row in &measurements.rows {
                assert_eq!(
                    row, &expected,
                    "case {case}: expected the row {expected:?}, got {row:?}"
                );
            }
        }
    }

    /// A superposed circuit reports only the outcomes its state allows, and each shot is
    /// drawn independently: the Bell state's two bits always agree, and over enough
    /// shots both of its outcomes appear.
    #[test]
    fn a_superposed_circuit_reports_only_the_outcomes_its_state_allows() {
        let bell = program(
            "OPENQASM 3.0; bit[2] b; qubit[2] q; h q[0]; cnot q[0], q[1]; \
             b[0] = measure q[0]; b[1] = measure q[1];",
        );
        let measurements = run(&bell, 200).expect("the Bell circuit runs");
        assert_eq!(
            measurements.measured_qubits,
            vec![0, 1],
            "case the Bell circuit: expected both qubits measured, got {measurements:?}"
        );
        let mut both_zero = 0;
        let mut both_one = 0;
        for row in &measurements.rows {
            assert_eq!(
                row[0], row[1],
                "case the Bell circuit: expected correlated bits, got {row:?}"
            );
            if row[0] == 0 {
                both_zero += 1;
            } else {
                both_one += 1;
            }
        }
        assert!(
            both_zero > 0 && both_one > 0,
            "case the Bell circuit over 200 shots: expected both outcomes, got \
             {both_zero} of 00 and {both_one} of 11"
        );
    }

    /// The work one run may cost is bounded by the PRODUCT of the state's size and the
    /// gates applied to it, so a program inside the bound runs and one past it is
    /// reported with the figures that exceeded it.
    #[test]
    fn a_run_past_the_work_bound_is_reported_rather_than_run() {
        // Sixteen qubits broadcast one gate each: well inside the bound.
        let inside = program("OPENQASM 3.0; qubit[16] q; h q;");
        let measurements = run(&inside, 4).expect("a run inside the bound is made");
        assert_eq!(
            measurements.rows.len(),
            4,
            "case inside the bound: expected one row per shot, got {:?}",
            measurements.rows
        );
        // Twenty qubits broadcast one gate each: 20 x 2^20 amplitude updates, past it.
        let past = program("OPENQASM 3.0; qubit[20] q; h q;");
        let refusal = run(&past, 4).expect_err("a run past the bound is refused");
        assert!(
            refusal.reason().contains("more work than the"),
            "case past the bound: expected the work bound in the reason, got {refusal:?}"
        );
    }

    /// A gate this simulator applies no matrix for stops the run with the reason, rather
    /// than being dropped from the circuit.
    #[test]
    fn a_gate_the_simulator_cannot_apply_stops_the_run() {
        // (case, source, a fragment of the reason)
        let cases: Vec<(&str, &str, &str)> = vec![
            (
                "a gate name nothing defines",
                "OPENQASM 3.0; qubit[1] q; frobnicate q[0];",
                "does not apply the gate frobnicate",
            ),
            (
                "a gate given the wrong parameter count",
                "OPENQASM 3.0; qubit[1] q; rx(0.1, 0.2) q[0];",
                "does not apply the gate rx",
            ),
            (
                "a two-qubit gate named with one qubit",
                "OPENQASM 3.0; qubit[2] q; cnot q[0];",
                "acts on 2 qubit(s)",
            ),
        ];
        for (case, source, fragment) in cases {
            let action = serde_json::json!({
                "braketSchemaHeader": { "name": crate::state::ACTION_OPENQASM, "version": "1" },
                "source": source,
            })
            .to_string();
            let lowered = gate_model_program(&action).expect("the program lowers");
            let refusal = run(&lowered, 4).expect_err(&format!("case {case}: expected a refusal"));
            assert!(
                refusal.reason().contains(fragment),
                "case {case}: expected a reason carrying {fragment:?}, got {refusal:?}"
            );
        }
    }
}
