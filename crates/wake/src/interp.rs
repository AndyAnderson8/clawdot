//! The interpreter: plans the tensor arena from the model and runs the operators (spec 0007 REQ-WAK-001,
//! REQ-WAK-003, REQ-WAK-005).
//!
//! Loading validates every operator against what the kernels implement and resolves every tensor to a byte range
//! of the arena, of the model file (constants) or of a variable buffer. Running does no allocation and no
//! lookup by name. Streaming state lives in variable buffers, initialised by the model's `CALL_ONCE` subgraph.

use std::ops::Range;

use crate::error::{Error, Result};
use crate::kernels::{
    ConvGeometry, QuantParams, Requant, Rounding, checked_requant, conv2d, depthwise_conv2d,
    fully_connected, logistic_table, multiply_by_quantized, out_size,
};
use crate::model::{Activation, Model, OpKind, Operator, Options, Subgraph, Tensor, TensorType};

/// Largest arena plus variables.
pub const MAX_MEMORY: usize = 1 << 20;
/// Most multiply-accumulates one inference may need (the official models need about 36 thousand).
pub const MAX_MACS: u64 = 1 << 24;
/// Most tensors one data-movement operator may join or split.
const FAN: usize = 5;

type Bytes = (usize, usize);

#[derive(Debug, Clone)]
struct Conv {
    geometry: ConvGeometry,
    quant: QuantParams,
    input: Range<usize>,
    output: Range<usize>,
    filter: Bytes,
    bias: Option<Bytes>,
    depthwise: bool,
}

#[derive(Debug, Clone)]
struct Fc {
    batches: usize,
    depth: usize,
    out_c: usize,
    quant: QuantParams,
    input: Range<usize>,
    output: Range<usize>,
    weights: Bytes,
    bias: Option<Bytes>,
}

#[derive(Debug, Clone)]
struct Slice {
    input: Range<usize>,
    output: Range<usize>,
    in_shape: [usize; 4],
    begin: [usize; 4],
    stride: [usize; 4],
    out_shape: [usize; 4],
}

#[derive(Debug, Clone)]
enum Step {
    Conv(Box<Conv>),
    Fc(Box<Fc>),
    Copy {
        src: Range<usize>,
        dst: Range<usize>,
    },
    /// Join: for each of `outer` rows, copy each input's chunk in order.
    Concat {
        inputs: Vec<(Range<usize>, usize)>,
        output: Range<usize>,
        outer: usize,
    },
    /// Split: for each of `outer` rows, copy each output's chunk in order out of the input.
    Split {
        input: Range<usize>,
        outputs: Vec<(Range<usize>, usize)>,
        outer: usize,
    },
    Slice(Box<Slice>),
    Logistic {
        input: Range<usize>,
        output: Range<usize>,
        table: Box<[i8; 256]>,
    },
    Quantize {
        input: Range<usize>,
        output: Range<usize>,
        input_zp: i32,
        output_zp: i32,
        requant: Requant,
        lo: i32,
        hi: i32,
    },
    ReadVar {
        var: usize,
        dst: Range<usize>,
    },
    AssignVar {
        var: usize,
        src: Range<usize>,
    },
}

/// Where a tensor lives.
#[derive(Debug, Clone)]
enum Loc {
    Arena(Range<usize>),
    Const,
    Handle(usize),
}

/// A planned, runnable model.
#[derive(Debug, Clone)]
pub struct Interpreter {
    model: Model,
    steps: Vec<Step>,
    arena: Vec<i8>,
    vars: Vec<Vec<i8>>,
    var_init: Vec<Option<Bytes>>,
    input: Range<usize>,
    input_quant: (f32, i32),
    output: Range<usize>,
    output_quant: (f32, i32),
    output_unsigned: bool,
    step_outputs: Vec<Vec<(usize, Range<usize>)>>,
    trace_ids: Vec<usize>,
    traced: Vec<(usize, Vec<i8>)>,
}

/// Whether the arena and all variables fit in [`MAX_MEMORY`]. The sizes are summed with saturation and nothing is
/// allocated here.
fn check_variable_memory(arena_len: usize, sizes: &[Option<usize>]) -> Result<()> {
    let total = sizes
        .iter()
        .fold(arena_len, |t, s| t.saturating_add(s.unwrap_or(0)));
    if total > MAX_MEMORY {
        return Err(Error::Limit("tensor arena and variables over 1 MiB"));
    }
    Ok(())
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

fn unsupported(msg: impl Into<String>) -> Error {
    Error::Unsupported(msg.into())
}

fn tensor<'a>(sg: &'a Subgraph, i: i32, what: &str) -> Result<&'a Tensor> {
    usize::try_from(i)
        .ok()
        .and_then(|u| sg.tensors.get(u))
        .ok_or_else(|| invalid(format!("{what}: missing tensor")))
}

fn input_at(op: &Operator, i: usize) -> Result<i32> {
    op.inputs
        .get(i)
        .copied()
        .ok_or_else(|| invalid(format!("{:?} is missing input {i}", op.kind)))
}

fn output_at(op: &Operator, i: usize) -> Result<i32> {
    op.outputs
        .get(i)
        .copied()
        .ok_or_else(|| invalid(format!("{:?} is missing output {i}", op.kind)))
}

/// The scale and zero point of a per-tensor quantised tensor.
fn scalar_quant(t: &Tensor, what: &str) -> Result<(f32, i32)> {
    if t.quant.scale.len() != 1 || t.quant.zero_point.len() != 1 {
        return Err(unsupported(format!(
            "{what}: needs one scale and one zero point"
        )));
    }
    let zp = t.quant.zero_point[0];
    let lo = if t.ty == TensorType::UInt8 { 0 } else { -128 };
    let hi = if t.ty == TensorType::UInt8 { 255 } else { 127 };
    if zp < lo || zp > hi {
        return Err(invalid(format!("{what}: zero point {zp} is out of range")));
    }
    Ok((t.quant.scale[0], zp as i32))
}

fn dims4(t: &Tensor, what: &str) -> Result<[usize; 4]> {
    match t.shape.as_slice() {
        [a, b, c, d] => Ok([*a, *b, *c, *d]),
        _ => Err(invalid(format!("{what}: expected a rank 4 tensor"))),
    }
}

fn activation_range(a: Activation, zp: i32) -> (i32, i32) {
    match a {
        Activation::None => (-128, 127),
        Activation::Relu => (zp.max(-128), 127),
    }
}

/// Per-channel filter scales: one per output channel along `dim`, or a single one. Zero points must be zero.
fn filter_scales(t: &Tensor, channels: usize, dim: usize, what: &str) -> Result<Vec<f32>> {
    if t.ty != TensorType::Int8 {
        return Err(unsupported(format!("{what}: filter is not int8")));
    }
    if t.quant.zero_point.iter().any(|&z| z != 0) {
        return Err(unsupported(format!(
            "{what}: filter with a non-zero zero point"
        )));
    }
    let n = t.quant.scale.len();
    if n == 1 {
        return Ok(t.quant.scale.clone());
    }
    if n != channels || t.quant.dim != dim {
        return Err(invalid(format!(
            "{what}: {n} filter scales for {channels} channels on axis {dim}"
        )));
    }
    Ok(t.quant.scale.clone())
}

struct Plan<'a> {
    model: &'a Model,
    sg: &'a Subgraph,
    loc: Vec<Option<Loc>>,
    rounding: Rounding,
    macs: std::cell::Cell<u64>,
}

impl Plan<'_> {
    fn arena(&self, i: i32, what: &str) -> Result<Range<usize>> {
        match usize::try_from(i).ok().and_then(|u| self.loc.get(u)) {
            Some(Some(Loc::Arena(r))) => Ok(r.clone()),
            _ => Err(invalid(format!("{what}: tensor {i} is not an activation"))),
        }
    }

    fn constant(&self, i: i32, what: &str) -> Result<(&Tensor, Bytes)> {
        let t = tensor(self.sg, i, what)?;
        match (t.data, self.model.data(t)) {
            (Some(range), Some(_)) => Ok((t, range)),
            _ => Err(invalid(format!("{what}: tensor {i} has no constant data"))),
        }
    }

    fn const_i32s(&self, i: i32, what: &str) -> Result<Vec<i32>> {
        let (t, range) = self.constant(i, what)?;
        if t.ty != TensorType::Int32 {
            return Err(invalid(format!("{what}: expected int32 data")));
        }
        let bytes = self
            .model
            .file()
            .get(range.0..range.0 + range.1)
            .ok_or(Error::Malformed("constant range"))?;
        Ok(bytes
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }
}

fn normalise_axis(axis: i32, rank: usize, what: &str) -> Result<usize> {
    let r = rank as i32;
    let a = if axis < 0 { axis + r } else { axis };
    if a < 0 || a >= r {
        return Err(invalid(format!(
            "{what}: axis {axis} is out of range for rank {rank}"
        )));
    }
    Ok(a as usize)
}

fn same_quant(a: &Tensor, b: &Tensor, what: &str) -> Result<()> {
    if a.ty != TensorType::Int8 || b.ty != TensorType::Int8 {
        return Err(unsupported(format!("{what}: only int8 tensors")));
    }
    if a.quant.scale != b.quant.scale || a.quant.zero_point != b.quant.zero_point {
        return Err(unsupported(format!(
            "{what}: inputs and output with different quantisation"
        )));
    }
    Ok(())
}

/// Variables by shared name: their sizes (known once an operator uses them) and initial values.
#[derive(Default)]
struct Vars {
    names: Vec<Vec<u8>>,
    sizes: Vec<Option<usize>>,
    init: Vec<Option<Bytes>>,
}

impl Vars {
    /// The index of the variable with this shared name, adding it on first sight.
    fn index_for(&mut self, name: &[u8]) -> usize {
        if let Some(p) = self.names.iter().position(|n| n == name) {
            return p;
        }
        self.names.push(name.to_vec());
        self.sizes.push(None);
        self.init.push(None);
        self.names.len() - 1
    }
}

/// Mark the handle tensors of the main graph and return the initialisation subgraphs it calls.
fn bind_handles(sg: &Subgraph, loc: &mut [Option<Loc>], vars: &mut Vars) -> Result<Vec<usize>> {
    let mut init_graphs: Vec<usize> = Vec::new();
    for op in &sg.operators {
        match (op.kind, &op.options) {
            (OpKind::VarHandle, Options::VarHandle { shared_name }) => {
                let out = output_at(op, 0)?;
                let v = vars.index_for(shared_name);
                let idx = usize::try_from(out).map_err(|_| invalid("VAR_HANDLE output"))?;
                if tensor(sg, out, "VAR_HANDLE")?.ty != TensorType::Resource {
                    return Err(invalid("VAR_HANDLE output is not a resource"));
                }
                loc[idx] = Some(Loc::Handle(v));
            }
            (OpKind::CallOnce, Options::CallOnce { init_subgraph }) => {
                init_graphs.push(*init_subgraph)
            }
            _ => {}
        }
    }
    Ok(init_graphs)
}

/// Run the initialisation subgraphs symbolically: their assigns give the initial value of each variable.
fn read_initial_values(model: &Model, init_graphs: &[usize], vars: &mut Vars) -> Result<()> {
    for &gi in init_graphs {
        let g = model
            .subgraphs
            .get(gi)
            .ok_or_else(|| invalid("CALL_ONCE names a missing subgraph"))?;
        let mut handles: Vec<Option<usize>> = vec![None; g.tensors.len()];
        for op in &g.operators {
            match (op.kind, &op.options) {
                (OpKind::VarHandle, Options::VarHandle { shared_name }) => {
                    let out = usize::try_from(output_at(op, 0)?)
                        .map_err(|_| invalid("VAR_HANDLE output"))?;
                    let v = vars.index_for(shared_name);
                    if let Some(h) = handles.get_mut(out) {
                        *h = Some(v);
                    }
                }
                (OpKind::AssignVariable, _) => {
                    let h = usize::try_from(input_at(op, 0)?)
                        .ok()
                        .and_then(|u| handles.get(u).copied().flatten());
                    let value = tensor(g, input_at(op, 1)?, "ASSIGN_VARIABLE")?;
                    let (Some(v), Some(range)) = (h, value.data) else {
                        return Err(invalid("an initial variable value must be constant"));
                    };
                    if value.ty != TensorType::Int8 || range.1 != value.elements() {
                        return Err(unsupported(
                            "variable initial values must be int8 constants",
                        ));
                    }
                    if let Some(slot) = vars.init.get_mut(v) {
                        *slot = Some(range);
                    }
                }
                _ => {
                    return Err(unsupported(format!(
                        "{:?} in an initialisation subgraph",
                        op.kind
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Tensors with constant data that are not handles live in the model file.
fn classify_constants(sg: &Subgraph, loc: &mut [Option<Loc>]) {
    for (i, t) in sg.tensors.iter().enumerate() {
        if loc[i].is_some() {
            continue;
        }
        if t.data.is_some() {
            loc[i] = Some(Loc::Const);
        } else if t.ty == TensorType::Resource {
            loc[i] = None;
        }
    }
}

/// The index of the last operator that reads each tensor (the model output lives to the end), checking that every
/// tensor is written once and before it is read.
fn tensor_lifetimes(
    sg: &Subgraph,
    loc: &[Option<Loc>],
    flow: &[&Operator],
    input_id: usize,
    output_id: usize,
) -> Result<Vec<usize>> {
    let mut def: Vec<Option<usize>> = vec![None; sg.tensors.len()];
    let mut last: Vec<usize> = vec![0; sg.tensors.len()];
    def[input_id] = Some(0);
    for (k, op) in flow.iter().enumerate() {
        for &t in &op.inputs {
            if let Ok(u) = usize::try_from(t) {
                if matches!(loc.get(u), Some(Some(Loc::Const | Loc::Handle(_)))) {
                    continue;
                }
                if def[u].is_none() {
                    return Err(invalid(format!("tensor {u} is read before it is written")));
                }
                last[u] = last[u].max(k);
            }
        }
        for &t in &op.outputs {
            let u = usize::try_from(t).map_err(|_| invalid("negative output tensor"))?;
            if matches!(loc.get(u), Some(Some(Loc::Const | Loc::Handle(_)))) {
                return Err(invalid("an operator writes a constant"));
            }
            if def[u].is_some() {
                return Err(invalid(format!("tensor {u} is written twice")));
            }
            def[u] = Some(k);
            last[u] = last[u].max(k);
        }
    }
    last[output_id] = flow.len();
    Ok(last)
}

/// First-fit allocation by lifetime. An allocation is free once its last reader has run, so an operator's output
/// never overlaps its own inputs. Fills in `loc` for the input and every operator output and returns the arena size.
fn allocate_arena(
    sg: &Subgraph,
    loc: &mut [Option<Loc>],
    flow: &[&Operator],
    last: &[usize],
    input_id: usize,
) -> Result<usize> {
    let mut live: Vec<(usize, usize, usize)> = Vec::new(); // (start, end, last use)
    let mut arena_len = 0usize;
    let mut allocate = |t: usize, at: usize| -> Result<Range<usize>> {
        let elems = sg.tensors[t].elements();
        let size = match sg.tensors[t].ty {
            TensorType::Int8 | TensorType::UInt8 => elems,
            _ => {
                return Err(unsupported(format!(
                    "activation tensor {t} is not int8 or uint8"
                )));
            }
        };
        live.retain(|&(_, _, l)| l >= at);
        live.sort_unstable();
        let mut start = 0usize;
        for &(s, e, _) in live.iter() {
            if start + size <= s {
                break;
            }
            start = start.max(e);
        }
        let end = start + size;
        live.push((start, end, last[t]));
        arena_len = arena_len.max(end);
        if arena_len > MAX_MEMORY {
            return Err(Error::Limit("tensor arena over 1 MiB"));
        }
        Ok(start..end)
    };
    loc[input_id] = Some(Loc::Arena(allocate(input_id, 0)?));
    for (k, op) in flow.iter().enumerate() {
        for &t in &op.outputs {
            let u = t as usize;
            loc[u] = Some(Loc::Arena(allocate(u, k)?));
        }
    }
    Ok(arena_len)
}

impl Interpreter {
    /// Parse, validate and plan a model.
    pub fn new(file: Vec<u8>) -> Result<Self> {
        Self::with_rounding(file, Rounding::Reference)
    }

    /// Parse, validate and plan a model with the given accumulator rounding (see [`Rounding`]).
    pub fn with_rounding(file: Vec<u8>, rounding: Rounding) -> Result<Self> {
        let model = Model::parse(file)?;
        Self::from_model(model, rounding)
    }

    fn from_model(model: Model, rounding: Rounding) -> Result<Self> {
        let sg = model
            .subgraphs
            .first()
            .ok_or(Error::Malformed("no subgraph"))?;
        let [input_id] = sg.inputs.as_slice() else {
            return Err(unsupported("the model must have exactly one input"));
        };
        let [output_id] = sg.outputs.as_slice() else {
            return Err(unsupported("the model must have exactly one output"));
        };
        let (input_id, output_id) = (*input_id, *output_id);

        let mut vars = Vars::default();
        let mut loc: Vec<Option<Loc>> = vec![None; sg.tensors.len()];
        let init_graphs = bind_handles(sg, &mut loc, &mut vars)?;
        read_initial_values(&model, &init_graphs, &mut vars)?;
        classify_constants(sg, &mut loc);

        // Which data-flow operators run, and the lifetime of every activation tensor.
        let flow: Vec<&Operator> = sg
            .operators
            .iter()
            .filter(|o| !matches!(o.kind, OpKind::VarHandle | OpKind::CallOnce))
            .collect();
        let n_ops = flow.len();
        let last = tensor_lifetimes(sg, &loc, &flow, input_id, output_id)?;
        let arena_len = allocate_arena(sg, &mut loc, &flow, &last, input_id)?;
        let Vars {
            sizes: var_sizes,
            init: var_init,
            ..
        } = vars;

        // Build the steps.
        let plan = Plan {
            model: &model,
            sg,
            loc,
            rounding,
            macs: std::cell::Cell::new(0),
        };
        let mut steps = Vec::with_capacity(n_ops);
        let mut step_outputs = Vec::with_capacity(n_ops);
        let mut sizes = var_sizes;
        for op in &flow {
            steps.push(plan.step(op, &mut sizes)?);
            let outs = op
                .outputs
                .iter()
                .filter_map(|&t| {
                    let u = usize::try_from(t).ok()?;
                    match plan.loc.get(u) {
                        Some(Some(Loc::Arena(r))) => Some((u, r.clone())),
                        _ => None,
                    }
                })
                .collect();
            step_outputs.push(outs);
        }

        // Variable buffers.
        for (i, s) in sizes.iter().enumerate() {
            if let Some((_, len)) = var_init.get(i).copied().flatten() {
                if len != s.unwrap_or(0) {
                    return Err(invalid(
                        "a variable's initial value differs in size from the variable",
                    ));
                }
            }
        }
        // Check the total before allocating anything: a bad model must not be able to abort the process
        // (REQ-WAK-002).
        check_variable_memory(arena_len, &sizes)?;
        let vars: Vec<Vec<i8>> = sizes.iter().map(|s| vec![0i8; s.unwrap_or(0)]).collect();

        // Input and output tensors.
        let in_t = &sg.tensors[input_id];
        let out_t = &sg.tensors[output_id];
        if in_t.ty != TensorType::Int8 {
            return Err(unsupported("the model input must be int8"));
        }
        if !matches!(out_t.ty, TensorType::Int8 | TensorType::UInt8) || out_t.elements() != 1 {
            return Err(unsupported(
                "the model output must be one int8 or uint8 value",
            ));
        }
        let input_quant = scalar_quant(in_t, "model input")?;
        let output_quant = scalar_quant(out_t, "model output")?;
        let input = match &plan.loc[input_id] {
            Some(Loc::Arena(r)) => r.clone(),
            _ => return Err(invalid("model input has no storage")),
        };
        let output = match &plan.loc[output_id] {
            Some(Loc::Arena(r)) => r.clone(),
            _ => return Err(invalid("model output is never written")),
        };
        let mut me = Self {
            model: model.clone(),
            steps,
            arena: vec![0i8; arena_len],
            vars,
            var_init,
            input,
            input_quant,
            output,
            output_quant,
            output_unsigned: out_t.ty == TensorType::UInt8,
            step_outputs,
            trace_ids: Vec::new(),
            traced: Vec::new(),
        };
        me.reset();
        Ok(me)
    }

    /// Restore the streaming state to the model's initial values.
    pub fn reset(&mut self) {
        for (v, init) in self.vars.iter_mut().zip(self.var_init.iter()) {
            match init.and_then(|(s, l)| self.model.file().get(s..s + l)) {
                Some(bytes) => {
                    for (d, &b) in v.iter_mut().zip(bytes.iter()) {
                        *d = b as i8;
                    }
                }
                None => v.fill(0),
            }
        }
    }

    /// Scale and zero point the model expects for its input.
    #[must_use]
    pub fn input_quant(&self) -> (f32, i32) {
        self.input_quant
    }

    /// Number of int8 values in the model input.
    #[must_use]
    pub fn input_len(&self) -> usize {
        self.input.len()
    }

    /// Shape of the model input.
    #[must_use]
    pub fn input_shape(&self) -> &[usize] {
        self.model.subgraphs.first().map_or(&[][..], |g| {
            g.inputs
                .first()
                .and_then(|&i| g.tensors.get(i))
                .map_or(&[][..], |t| t.shape.as_slice())
        })
    }

    /// Working memory in bytes: the arena and the streaming state (the model file is extra).
    #[must_use]
    pub fn working_bytes(&self) -> usize {
        self.arena.len() + self.vars.iter().map(Vec::len).sum::<usize>()
    }

    /// Size of the model file kept in memory.
    #[must_use]
    pub fn model_bytes(&self) -> usize {
        self.model.file().len()
    }

    /// Run one inference. `input` has [`Interpreter::input_len`] values.
    pub fn invoke(&mut self, input: &[i8]) -> Result<()> {
        if input.len() != self.input.len() {
            return Err(invalid("input length does not match the model"));
        }
        let Self {
            model,
            steps,
            arena,
            vars,
            input: in_range,
            step_outputs,
            trace_ids,
            traced,
            ..
        } = self;
        arena
            .get_mut(in_range.clone())
            .ok_or(Error::Malformed("input range"))?
            .copy_from_slice(input);
        let file = model.file();
        for (step, outs) in steps.iter().zip(step_outputs.iter()) {
            run_step(step, file, arena, vars)?;
            if !trace_ids.is_empty() {
                for (id, range) in outs {
                    if trace_ids.contains(id) {
                        if let Some(data) = arena.get(range.clone()) {
                            traced.push((*id, data.to_vec()));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Record the named tensors (by their index in the model) as they are produced, for comparing against
    /// another implementation tensor by tensor. Allocates while on; leave it off in normal use.
    pub fn trace(&mut self, tensors: &[usize]) {
        self.trace_ids = tensors.to_vec();
        self.traced.clear();
    }

    /// The tensors recorded since [`Interpreter::trace`], in production order, and clear the record.
    pub fn take_traced(&mut self) -> Vec<(usize, Vec<i8>)> {
        std::mem::take(&mut self.traced)
    }

    /// The raw output value of the last inference.
    #[must_use]
    pub fn output_raw(&self) -> i32 {
        let v = self.arena.get(self.output.start).copied().unwrap_or(0);
        if self.output_unsigned {
            i32::from(v as u8)
        } else {
            i32::from(v)
        }
    }

    /// The output of the last inference as a real number (`scale * (q - zero_point)`).
    #[must_use]
    pub fn output_value(&self) -> f32 {
        (self.output_raw() - self.output_quant.1) as f32 * self.output_quant.0
    }
}

impl Plan<'_> {
    /// Count `outputs * per_output` multiply-accumulates against the limit as soon as an operator is planned.
    fn account(&self, outputs: u64, per_output: u64) -> Result<()> {
        let total = self
            .macs
            .get()
            .saturating_add(outputs.saturating_mul(per_output));
        self.macs.set(total);
        if total > MAX_MACS {
            return Err(Error::Limit(
                "more than 16 million multiply-accumulates per inference",
            ));
        }
        Ok(())
    }

    fn step(&self, op: &Operator, var_sizes: &mut [Option<usize>]) -> Result<Step> {
        let what = format!("{:?}", op.kind);
        let what = what.as_str();
        match op.kind {
            OpKind::Conv2d | OpKind::DepthwiseConv2d => self.conv(op, what),
            OpKind::FullyConnected => self.fully_connected(op, what),
            OpKind::Reshape => self.reshape(op, what),
            OpKind::Concatenation => self.concatenation(op, what),
            OpKind::SplitV => self.split_v(op, what),
            OpKind::StridedSlice => self.strided_slice(op, what),
            OpKind::Logistic => self.logistic(op, what),
            OpKind::Quantize => self.quantize(op, what),
            OpKind::ReadVariable | OpKind::AssignVariable => {
                self.variable_access(op, what, var_sizes)
            }
            OpKind::VarHandle | OpKind::CallOnce => Err(invalid("handled earlier")),
        }
    }

    fn fully_connected(&self, op: &Operator, what: &str) -> Result<Step> {
        let sg = self.sg;
        let Options::FullyConnected { activation } = op.options else {
            return Err(invalid("FULLY_CONNECTED options"));
        };
        let (in_i, w_i, out_i) = (input_at(op, 0)?, input_at(op, 1)?, output_at(op, 0)?);
        let (it, wt, ot) = (
            tensor(sg, in_i, what)?,
            tensor(sg, w_i, what)?,
            tensor(sg, out_i, what)?,
        );
        let (w_t, weights) = self.constant(w_i, what)?;
        let [out_c, depth] = *wt.shape.as_slice() else {
            return Err(invalid("FULLY_CONNECTED weights must be rank 2"));
        };
        if depth == 0 || it.elements() % depth != 0 {
            return Err(invalid(
                "FULLY_CONNECTED input does not divide into the weight depth",
            ));
        }
        let batches = it.elements() / depth;
        if ot.elements() != batches * out_c {
            return Err(invalid("FULLY_CONNECTED output size mismatch"));
        }
        let scales = filter_scales(w_t, out_c, 0, what)?;
        if scales.len() != 1 {
            return Err(unsupported("FULLY_CONNECTED with per-channel weights"));
        }
        let (in_scale, in_zp) = scalar_quant(it, what)?;
        let (out_scale, out_zp) = scalar_quant(ot, what)?;
        let (lo, hi) = activation_range(activation, out_zp);
        let bias = self.bias(op, 2, out_c, what)?;
        self.account((batches * out_c) as u64, depth as u64)?;
        Ok(Step::Fc(Box::new(Fc {
            batches,
            depth,
            out_c,
            quant: QuantParams {
                input_zp: in_zp,
                output_zp: out_zp,
                act_min: lo,
                act_max: hi,
                requant: vec![checked_requant(
                    f64::from(in_scale) * f64::from(scales[0]) / f64::from(out_scale),
                )?],
                rounding: self.rounding,
            },
            input: self.arena(in_i, what)?,
            output: self.arena(out_i, what)?,
            weights,
            bias,
        })))
    }

    fn reshape(&self, op: &Operator, what: &str) -> Result<Step> {
        let sg = self.sg;
        let (in_i, out_i) = (input_at(op, 0)?, output_at(op, 0)?);
        let (it, ot) = (tensor(sg, in_i, what)?, tensor(sg, out_i, what)?);
        same_quant(it, ot, what)?;
        if it.elements() != ot.elements() {
            return Err(invalid("RESHAPE changes the number of elements"));
        }
        Ok(Step::Copy {
            src: self.arena(in_i, what)?,
            dst: self.arena(out_i, what)?,
        })
    }

    fn concatenation(&self, op: &Operator, what: &str) -> Result<Step> {
        let sg = self.sg;
        let Options::Concat { axis, activation } = op.options else {
            return Err(invalid("CONCATENATION options"));
        };
        if activation != Activation::None {
            return Err(unsupported("CONCATENATION with a fused activation"));
        }
        if op.inputs.is_empty() || op.inputs.len() > FAN {
            return Err(unsupported("CONCATENATION fan-in above 5"));
        }
        let out_i = output_at(op, 0)?;
        let ot = tensor(sg, out_i, what)?;
        let rank = ot.shape.len();
        let ax = normalise_axis(axis, rank, what)?;
        let outer: usize = ot.shape[..ax].iter().product();
        let inner: usize = ot.shape[ax + 1..].iter().product();
        let mut total_axis = 0usize;
        let mut inputs = Vec::new();
        for &i in &op.inputs {
            let it = tensor(sg, i, what)?;
            same_quant(it, ot, what)?;
            if it.shape.len() != rank
                || it.shape[..ax] != ot.shape[..ax]
                || it.shape[ax + 1..] != ot.shape[ax + 1..]
            {
                return Err(invalid("CONCATENATION input shapes disagree"));
            }
            total_axis += it.shape[ax];
            inputs.push((self.arena(i, what)?, it.shape[ax] * inner));
        }
        if total_axis != ot.shape[ax] {
            return Err(invalid("CONCATENATION output size mismatch"));
        }
        Ok(Step::Concat {
            inputs,
            output: self.arena(out_i, what)?,
            outer,
        })
    }

    fn split_v(&self, op: &Operator, what: &str) -> Result<Step> {
        let sg = self.sg;
        let Options::SplitV { num_splits } = op.options else {
            return Err(invalid("SPLIT_V options"));
        };
        if num_splits != op.outputs.len() || num_splits > FAN {
            return Err(unsupported("SPLIT_V fan-out"));
        }
        let in_i = input_at(op, 0)?;
        let it = tensor(sg, in_i, what)?;
        let axis_v = self.const_i32s(input_at(op, 2)?, what)?;
        let axis = *axis_v.first().ok_or_else(|| invalid("SPLIT_V axis"))?;
        let rank = it.shape.len();
        let ax = normalise_axis(axis, rank, what)?;
        let outer: usize = it.shape[..ax].iter().product();
        let inner: usize = it.shape[ax + 1..].iter().product();
        let mut outputs = Vec::new();
        let mut sum = 0usize;
        for &o in &op.outputs {
            let ot = tensor(sg, o, what)?;
            same_quant(it, ot, what)?;
            if ot.shape.len() != rank
                || ot.shape[..ax] != it.shape[..ax]
                || ot.shape[ax + 1..] != it.shape[ax + 1..]
            {
                return Err(invalid("SPLIT_V output shape"));
            }
            sum += ot.shape[ax];
            outputs.push((self.arena(o, what)?, ot.shape[ax] * inner));
        }
        if sum != it.shape[ax] {
            return Err(invalid("SPLIT_V outputs do not add up to the input"));
        }
        Ok(Step::Split {
            input: self.arena(in_i, what)?,
            outputs,
            outer,
        })
    }

    fn logistic(&self, op: &Operator, what: &str) -> Result<Step> {
        let sg = self.sg;
        let (in_i, out_i) = (input_at(op, 0)?, output_at(op, 0)?);
        let (it, ot) = (tensor(sg, in_i, what)?, tensor(sg, out_i, what)?);
        if it.ty != TensorType::Int8 || ot.ty != TensorType::Int8 || it.elements() != ot.elements()
        {
            return Err(unsupported("LOGISTIC needs int8 tensors of equal size"));
        }
        let (in_scale, in_zp) = scalar_quant(it, what)?;
        let (out_scale, out_zp) = scalar_quant(ot, what)?;
        if (out_scale - 1.0 / 256.0).abs() > 1e-9 || out_zp != -128 {
            return Err(unsupported(
                "LOGISTIC output must have scale 1/256 and zero point -128",
            ));
        }
        Ok(Step::Logistic {
            input: self.arena(in_i, what)?,
            output: self.arena(out_i, what)?,
            table: Box::new(logistic_table(in_scale, in_zp)),
        })
    }

    fn quantize(&self, op: &Operator, what: &str) -> Result<Step> {
        let sg = self.sg;
        let (in_i, out_i) = (input_at(op, 0)?, output_at(op, 0)?);
        let (it, ot) = (tensor(sg, in_i, what)?, tensor(sg, out_i, what)?);
        if it.ty != TensorType::Int8
            || !matches!(ot.ty, TensorType::Int8 | TensorType::UInt8)
            || it.elements() != ot.elements()
        {
            return Err(unsupported("QUANTIZE from int8 to int8 or uint8 only"));
        }
        let (in_scale, in_zp) = scalar_quant(it, what)?;
        let (out_scale, out_zp) = scalar_quant(ot, what)?;
        let (lo, hi) = if ot.ty == TensorType::UInt8 {
            (0, 255)
        } else {
            (-128, 127)
        };
        Ok(Step::Quantize {
            input: self.arena(in_i, what)?,
            output: self.arena(out_i, what)?,
            input_zp: in_zp,
            output_zp: out_zp,
            requant: checked_requant(f64::from(in_scale) / f64::from(out_scale))?,
            lo,
            hi,
        })
    }

    fn variable_access(
        &self,
        op: &Operator,
        what: &str,
        var_sizes: &mut [Option<usize>],
    ) -> Result<Step> {
        let sg = self.sg;
        let handle = input_at(op, 0)?;
        let var = match usize::try_from(handle).ok().and_then(|u| self.loc.get(u)) {
            Some(Some(Loc::Handle(v))) => *v,
            _ => return Err(invalid(format!("{what}: not a variable handle"))),
        };
        let (value_i, reading) = if op.kind == OpKind::ReadVariable {
            (output_at(op, 0)?, true)
        } else {
            (input_at(op, 1)?, false)
        };
        let vt = tensor(sg, value_i, what)?;
        if vt.ty != TensorType::Int8 {
            return Err(unsupported(format!("{what}: only int8 variables")));
        }
        let slot = var_sizes
            .get_mut(var)
            .ok_or_else(|| invalid("variable index"))?;
        match slot {
            Some(n) if *n != vt.elements() => {
                return Err(invalid(format!("{what}: variable size mismatch")));
            }
            _ => *slot = Some(vt.elements()),
        }
        let range = self.arena(value_i, what)?;
        Ok(if reading {
            Step::ReadVar { var, dst: range }
        } else {
            Step::AssignVar { var, src: range }
        })
    }

    fn bias(&self, op: &Operator, at: usize, channels: usize, what: &str) -> Result<Option<Bytes>> {
        match op.inputs.get(at) {
            None | Some(-1) => Ok(None),
            Some(&b) => {
                let (bt, range) = self.constant(b, what)?;
                if bt.ty != TensorType::Int32 || bt.elements() != channels {
                    return Err(invalid(format!(
                        "{what}: bias must be {channels} int32 values"
                    )));
                }
                Ok(Some(range))
            }
        }
    }

    fn conv(&self, op: &Operator, what: &str) -> Result<Step> {
        let sg = self.sg;
        let depthwise = op.kind == OpKind::DepthwiseConv2d;
        let Options::Conv {
            padding,
            stride_w,
            stride_h,
            dilation_w,
            dilation_h,
            depth_multiplier,
            activation,
        } = op.options
        else {
            return Err(invalid("convolution options"));
        };
        let (in_i, f_i, out_i) = (input_at(op, 0)?, input_at(op, 1)?, output_at(op, 0)?);
        let (it, ft, ot) = (
            tensor(sg, in_i, what)?,
            tensor(sg, f_i, what)?,
            tensor(sg, out_i, what)?,
        );
        let (f_t, filter) = self.constant(f_i, what)?;
        let [n, in_h, in_w, in_c] = dims4(it, what)?;
        let [f0, k_h, k_w, f3] = dims4(ft, what)?;
        let out_c = if depthwise { f3 } else { f0 };
        if depthwise {
            if f0 != 1 || out_c != in_c * depth_multiplier {
                return Err(invalid("DEPTHWISE_CONV_2D filter shape"));
            }
        } else if f3 != in_c {
            return Err(invalid(
                "CONV_2D filter depth differs from the input channels",
            ));
        }
        if k_h == 0 || k_w == 0 {
            return Err(invalid("empty convolution kernel"));
        }
        let (out_h, pad_h) = out_size(in_h, k_h, stride_h, dilation_h, padding);
        let (out_w, pad_w) = out_size(in_w, k_w, stride_w, dilation_w, padding);
        if dims4(ot, what)? != [n, out_h, out_w, out_c] {
            return Err(invalid(format!(
                "{what}: output shape differs from the computed one"
            )));
        }
        let scales = filter_scales(f_t, out_c, if depthwise { 3 } else { 0 }, what)?;
        let (in_scale, in_zp) = scalar_quant(it, what)?;
        let (out_scale, out_zp) = scalar_quant(ot, what)?;
        if it.ty != TensorType::Int8 || ot.ty != TensorType::Int8 {
            return Err(unsupported(format!("{what}: only int8 activations")));
        }
        let (lo, hi) = activation_range(activation, out_zp);
        let requant = scales
            .iter()
            .map(|&s| checked_requant(f64::from(in_scale) * f64::from(s) / f64::from(out_scale)))
            .collect::<Result<Vec<_>>>()?;
        self.account(
            (n * out_h * out_w * out_c) as u64,
            (k_h * k_w * if depthwise { 1 } else { in_c }) as u64,
        )?;
        Ok(Step::Conv(Box::new(Conv {
            geometry: ConvGeometry {
                batch: n,
                in_h,
                in_w,
                in_c,
                out_h,
                out_w,
                out_c,
                k_h,
                k_w,
                stride_h,
                stride_w,
                dil_h: dilation_h,
                dil_w: dilation_w,
                pad_h,
                pad_w,
                depth_mult: if depthwise { depth_multiplier } else { 1 },
            },
            quant: QuantParams {
                input_zp: in_zp,
                output_zp: out_zp,
                act_min: lo,
                act_max: hi,
                requant,
                rounding: self.rounding,
            },
            input: self.arena(in_i, what)?,
            output: self.arena(out_i, what)?,
            filter,
            bias: self.bias(op, 2, out_c, what)?,
            depthwise,
        })))
    }

    fn strided_slice(&self, op: &Operator, what: &str) -> Result<Step> {
        let sg = self.sg;
        let Options::StridedSlice {
            begin_mask,
            end_mask,
            ellipsis_mask,
            new_axis_mask,
            shrink_axis_mask,
        } = op.options
        else {
            return Err(invalid("STRIDED_SLICE options"));
        };
        if ellipsis_mask != 0 || new_axis_mask != 0 {
            return Err(unsupported("STRIDED_SLICE with an ellipsis or new axes"));
        }
        let (in_i, out_i) = (input_at(op, 0)?, output_at(op, 0)?);
        let (it, ot) = (tensor(sg, in_i, what)?, tensor(sg, out_i, what)?);
        same_quant(it, ot, what)?;
        let begin = self.const_i32s(input_at(op, 1)?, what)?;
        let end = self.const_i32s(input_at(op, 2)?, what)?;
        let strides = self.const_i32s(input_at(op, 3)?, what)?;
        let rank = it.shape.len();
        if rank == 0
            || rank > 4
            || begin.len() != rank
            || end.len() != rank
            || strides.len() != rank
        {
            return Err(invalid(
                "STRIDED_SLICE parameter lengths differ from the input rank",
            ));
        }
        // Work on a rank 4 view with leading unit axes.
        let pad = 4 - rank;
        let mut in_shape = [1usize; 4];
        let mut b4 = [0usize; 4];
        let mut s4 = [1usize; 4];
        let mut o4 = [1usize; 4];
        for a in 0..rank {
            let dim = it.shape[a];
            let s = strides[a];
            if s <= 0 {
                return Err(unsupported("STRIDED_SLICE with a zero or negative stride"));
            }
            let norm = |v: i32| -> usize {
                let v = if v < 0 {
                    i64::from(v) + dim as i64
                } else {
                    i64::from(v)
                };
                v.clamp(0, dim as i64) as usize
            };
            let bi = if begin_mask >> a & 1 == 1 {
                0
            } else {
                norm(begin[a])
            };
            let ei = if shrink_axis_mask >> a & 1 == 1 {
                bi + 1
            } else if end_mask >> a & 1 == 1 {
                dim
            } else {
                norm(end[a])
            };
            let span = ei.saturating_sub(bi);
            in_shape[pad + a] = dim;
            b4[pad + a] = bi;
            s4[pad + a] = s as usize;
            o4[pad + a] = span.div_ceil(s as usize);
            if shrink_axis_mask >> a & 1 == 1 && (bi >= dim || o4[pad + a] != 1) {
                return Err(invalid("STRIDED_SLICE shrinks an axis outside the tensor"));
            }
        }
        if o4.iter().product::<usize>() != ot.elements() {
            return Err(invalid(
                "STRIDED_SLICE output size differs from the computed one",
            ));
        }
        Ok(Step::Slice(Box::new(Slice {
            input: self.arena(in_i, what)?,
            output: self.arena(out_i, what)?,
            in_shape,
            begin: b4,
            stride: s4,
            out_shape: o4,
        })))
    }
}

/// Split one buffer into disjoint mutable slices, one per range, in the order given. `None` if two overlap or one
/// runs past the end.
fn carve<'a>(buf: &'a mut [i8], ranges: &[&Range<usize>]) -> Option<[&'a mut [i8]; 2 * FAN]> {
    let n = ranges.len();
    if n > 2 * FAN {
        return None;
    }
    // Order by start with an insertion sort on a fixed array, so nothing is allocated while processing audio.
    let mut order = [0usize; 2 * FAN];
    for (i, o) in order.iter_mut().enumerate().take(n) {
        *o = i;
    }
    for i in 1..n {
        let mut j = i;
        while j > 0 && ranges[order[j - 1]].start > ranges[order[j]].start {
            order.swap(j - 1, j);
            j -= 1;
        }
    }
    let mut slots: [Option<&'a mut [i8]>; 2 * FAN] = std::array::from_fn(|_| None);
    let mut rest = buf;
    let mut cursor = 0usize;
    for &i in order.iter().take(n) {
        let r = ranges[i];
        if r.start < cursor || r.end < r.start {
            return None;
        }
        let tail = std::mem::take(&mut rest);
        let skip = r.start - cursor;
        if skip > tail.len() {
            return None;
        }
        let (_, after) = tail.split_at_mut(skip);
        if r.len() > after.len() {
            return None;
        }
        let (mine, remainder) = after.split_at_mut(r.len());
        slots[i] = Some(mine);
        rest = remainder;
        cursor = r.end;
    }
    Some(std::array::from_fn(|i| slots[i].take().unwrap_or_default()))
}

fn bytes(file: &[u8], b: Bytes) -> Result<&[u8]> {
    file.get(
        b.0..b
            .0
            .checked_add(b.1)
            .ok_or(Error::Malformed("constant range"))?,
    )
    .ok_or(Error::Malformed("constant range"))
}

fn overlap() -> Error {
    Error::Invalid("operator ranges overlap".into())
}

fn run_step(step: &Step, file: &[u8], arena: &mut [i8], vars: &mut [Vec<i8>]) -> Result<()> {
    match step {
        Step::Conv(c) => run_conv(c, file, arena)?,
        Step::Fc(f) => run_fc(f, file, arena)?,
        Step::Copy { src, dst } => {
            let [s, d, ..] = carve(arena, &[src, dst]).ok_or_else(overlap)?;
            if s.len() != d.len() {
                return Err(overlap());
            }
            d.copy_from_slice(s);
        }
        Step::Concat {
            inputs,
            output,
            outer,
        } => run_concat(inputs, output, *outer, arena)?,
        Step::Split {
            input,
            outputs,
            outer,
        } => run_split(input, outputs, *outer, arena)?,
        Step::Slice(s) => run_slice(s, arena)?,
        Step::Logistic {
            input,
            output,
            table,
        } => {
            let [i, o, ..] = carve(arena, &[input, output]).ok_or_else(overlap)?;
            for (d, &s) in o.iter_mut().zip(i.iter()) {
                *d = table[(i32::from(s) + 128) as usize];
            }
        }
        Step::Quantize {
            input,
            output,
            input_zp,
            output_zp,
            requant,
            lo,
            hi,
        } => {
            let [i, o, ..] = carve(arena, &[input, output]).ok_or_else(overlap)?;
            for (d, &s) in o.iter_mut().zip(i.iter()) {
                let v = multiply_by_quantized(i32::from(s) - input_zp, *requant)
                    .wrapping_add(*output_zp);
                *d = v.clamp(*lo, *hi) as u8 as i8;
            }
        }
        Step::ReadVar { var, dst } => {
            let v = vars.get(*var).ok_or_else(overlap)?;
            let d = arena.get_mut(dst.clone()).ok_or_else(overlap)?;
            if d.len() != v.len() {
                return Err(overlap());
            }
            d.copy_from_slice(v);
        }
        Step::AssignVar { var, src } => {
            let v = vars.get_mut(*var).ok_or_else(overlap)?;
            let s = arena.get(src.clone()).ok_or_else(overlap)?;
            if s.len() != v.len() {
                return Err(overlap());
            }
            v.copy_from_slice(s);
        }
    }
    Ok(())
}

fn run_conv(c: &Conv, file: &[u8], arena: &mut [i8]) -> Result<()> {
    let [i, o, ..] = carve(arena, &[&c.input, &c.output]).ok_or_else(overlap)?;
    let filter = bytes(file, c.filter)?;
    let bias = match c.bias {
        Some(b) => bytes(file, b)?,
        None => &[],
    };
    if c.depthwise {
        depthwise_conv2d(&c.geometry, &c.quant, i, filter, bias, o);
    } else {
        conv2d(&c.geometry, &c.quant, i, filter, bias, o);
    }
    Ok(())
}

fn run_fc(f: &Fc, file: &[u8], arena: &mut [i8]) -> Result<()> {
    let [i, o, ..] = carve(arena, &[&f.input, &f.output]).ok_or_else(overlap)?;
    let bias = match f.bias {
        Some(b) => bytes(file, b)?,
        None => &[],
    };
    fully_connected(
        f.batches,
        f.depth,
        f.out_c,
        &f.quant,
        i,
        bytes(file, f.weights)?,
        bias,
        o,
    );
    Ok(())
}

fn run_concat(
    inputs: &[(Range<usize>, usize)],
    output: &Range<usize>,
    outer: usize,
    arena: &mut [i8],
) -> Result<()> {
    let n = inputs.len();
    let mut ranges: [&Range<usize>; 2 * FAN] = [output; 2 * FAN];
    for (slot, (r, _)) in ranges.iter_mut().zip(inputs.iter()) {
        *slot = r;
    }
    ranges[n] = output;
    let mut slots = carve(arena, &ranges[..=n]).ok_or_else(overlap)?;
    let total: usize = inputs.iter().map(|(_, c)| *c).sum();
    // The inputs come first in the array order asked for, the output after them.
    let (ins, rest) = slots.split_at_mut(n);
    let out = rest.first_mut().ok_or_else(overlap)?;
    for o in 0..outer {
        let mut at = o * total;
        for (k, (_, chunk)) in inputs.iter().enumerate() {
            let src = ins
                .get(k)
                .and_then(|s| s.get(o * chunk..(o + 1) * chunk))
                .ok_or_else(overlap)?;
            out.get_mut(at..at + chunk)
                .ok_or_else(overlap)?
                .copy_from_slice(src);
            at += chunk;
        }
    }
    Ok(())
}

fn run_split(
    input: &Range<usize>,
    outputs: &[(Range<usize>, usize)],
    outer: usize,
    arena: &mut [i8],
) -> Result<()> {
    let n = outputs.len();
    let mut ranges: [&Range<usize>; 2 * FAN] = [input; 2 * FAN];
    for (slot, (r, _)) in ranges.iter_mut().skip(1).zip(outputs.iter()) {
        *slot = r;
    }
    let mut slots = carve(arena, &ranges[..=n]).ok_or_else(overlap)?;
    let total: usize = outputs.iter().map(|(_, c)| *c).sum();
    let (head, outs) = slots.split_at_mut(1);
    let inp = head.first().ok_or_else(overlap)?;
    for o in 0..outer {
        let mut at = o * total;
        for (k, (_, chunk)) in outputs.iter().enumerate() {
            let src = inp.get(at..at + chunk).ok_or_else(overlap)?;
            outs.get_mut(k)
                .and_then(|s| s.get_mut(o * chunk..(o + 1) * chunk))
                .ok_or_else(overlap)?
                .copy_from_slice(src);
            at += chunk;
        }
    }
    Ok(())
}

fn run_slice(s: &Slice, arena: &mut [i8]) -> Result<()> {
    let [i, o, ..] = carve(arena, &[&s.input, &s.output]).ok_or_else(overlap)?;
    let mut at = 0usize;
    for a in 0..s.out_shape[0] {
        for b in 0..s.out_shape[1] {
            for c in 0..s.out_shape[2] {
                for d in 0..s.out_shape[3] {
                    let idx = [a, b, c, d];
                    let mut off = 0usize;
                    #[allow(clippy::needless_range_loop)]
                    for ax in 0..4 {
                        off = off * s.in_shape[ax] + s.begin[ax] + idx[ax] * s.stride[ax];
                    }
                    *o.get_mut(at).ok_or_else(overlap)? = *i.get(off).ok_or_else(overlap)?;
                    at += 1;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_wak_002_variable_sizes_are_summed_and_checked_before_anything_is_allocated() {
        // These sizes would abort the process if a buffer were allocated for them; the check only adds.
        let huge = [Some(usize::MAX / 2), Some(usize::MAX / 2), Some(8)];
        assert!(matches!(
            check_variable_memory(0, &huge),
            Err(Error::Limit(_))
        ));
        assert!(matches!(
            check_variable_memory(MAX_MEMORY, &[Some(1)]),
            Err(Error::Limit(_))
        ));
        assert!(check_variable_memory(MAX_MEMORY - 8, &[Some(8), None]).is_ok());
    }
}
