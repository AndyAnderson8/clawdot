//! The TFLite schema, read into plain structs (spec 0007 REQ-WAK-001, REQ-WAK-002).
//!
//! Only what the streaming microWakeWord models need is read. Field numbers are the vtable slots of
//! `tensorflow/lite/schema/schema.fbs` (Apache-2.0): a fact about a file format, not copied code.
//! Constant data (weights, biases, shapes) stays in the owned file bytes and is referenced by range.

use crate::error::{Error, Result};
use crate::flat::{Buf, Table};

/// Largest model file accepted.
pub const MAX_FILE_BYTES: usize = 1 << 20;
/// Largest number of tensors, operators, buffers or subgraph inputs and outputs.
pub const MAX_ITEMS: usize = 1024;
/// Largest tensor rank.
pub const MAX_RANK: usize = 4;
/// Largest number of elements in one tensor.
pub const MAX_ELEMENTS: usize = 1 << 20;
/// Largest number of subgraphs.
pub const MAX_SUBGRAPHS: usize = 4;

/// Element type of a tensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TensorType {
    /// Signed 8-bit.
    Int8,
    /// Unsigned 8-bit.
    UInt8,
    /// Signed 32-bit (biases, shapes, indices).
    Int32,
    /// A variable handle.
    Resource,
    /// Anything else, by the schema's code.
    Other(i8),
}

impl TensorType {
    fn from_code(c: i8) -> Self {
        match c {
            9 => Self::Int8,
            3 => Self::UInt8,
            2 => Self::Int32,
            13 => Self::Resource,
            o => Self::Other(o),
        }
    }

    /// Bytes per element, where the type has data.
    #[must_use]
    pub fn size(self) -> Option<usize> {
        match self {
            Self::Int8 | Self::UInt8 => Some(1),
            Self::Int32 => Some(4),
            Self::Resource | Self::Other(_) => None,
        }
    }
}

/// Quantisation of one tensor: real = scale * (q - zero_point), per tensor or per channel.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quant {
    /// One scale, or one per channel along `dim`.
    pub scale: Vec<f32>,
    /// Zero points, same length as `scale` (or empty).
    pub zero_point: Vec<i64>,
    /// The channel axis for per-channel quantisation.
    pub dim: usize,
}

/// One tensor.
#[derive(Debug, Clone)]
pub struct Tensor {
    /// Dimensions (empty for a scalar).
    pub shape: Vec<usize>,
    /// Element type.
    pub ty: TensorType,
    /// Quantisation.
    pub quant: Quant,
    /// Constant data as a byte range of the model file, if the tensor has any.
    pub data: Option<(usize, usize)>,
    /// The tensor was declared a variable.
    pub is_variable: bool,
}

impl Tensor {
    /// Number of elements (1 for a scalar).
    #[must_use]
    pub fn elements(&self) -> usize {
        self.shape.iter().product()
    }
}

/// Padding scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Padding {
    /// Zero padding so the output size is `ceil(input / stride)`.
    Same,
    /// No padding.
    Valid,
}

/// Fused activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// None.
    None,
    /// `max(0, x)`.
    Relu,
}

/// The operators this runtime implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// `CONV_2D`
    Conv2d,
    /// `DEPTHWISE_CONV_2D`
    DepthwiseConv2d,
    /// `FULLY_CONNECTED`
    FullyConnected,
    /// `CONCATENATION`
    Concatenation,
    /// `STRIDED_SLICE`
    StridedSlice,
    /// `SPLIT_V`
    SplitV,
    /// `RESHAPE`
    Reshape,
    /// `LOGISTIC`
    Logistic,
    /// `QUANTIZE`
    Quantize,
    /// `CALL_ONCE`
    CallOnce,
    /// `VAR_HANDLE`
    VarHandle,
    /// `READ_VARIABLE`
    ReadVariable,
    /// `ASSIGN_VARIABLE`
    AssignVariable,
}

/// Per-operator options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Options {
    /// No options.
    None,
    /// `Conv2DOptions` and `DepthwiseConv2DOptions`.
    Conv {
        /// Padding scheme.
        padding: Padding,
        /// Horizontal stride (along width, axis 2).
        stride_w: usize,
        /// Vertical stride (along height, axis 1).
        stride_h: usize,
        /// Dilation along width.
        dilation_w: usize,
        /// Dilation along height.
        dilation_h: usize,
        /// Depth multiplier (depthwise only; 1 for CONV_2D).
        depth_multiplier: usize,
        /// Fused activation.
        activation: Activation,
    },
    /// `FullyConnectedOptions`.
    FullyConnected {
        /// Fused activation.
        activation: Activation,
    },
    /// `ConcatenationOptions`.
    Concat {
        /// Axis, possibly negative.
        axis: i32,
        /// Fused activation.
        activation: Activation,
    },
    /// `StridedSliceOptions`.
    StridedSlice {
        /// Bit per axis: ignore `begin`.
        begin_mask: i32,
        /// Bit per axis: ignore `end`.
        end_mask: i32,
        /// Must be zero.
        ellipsis_mask: i32,
        /// Must be zero.
        new_axis_mask: i32,
        /// Bit per axis: take one index and drop the axis.
        shrink_axis_mask: i32,
    },
    /// `SplitVOptions`.
    SplitV {
        /// Number of outputs.
        num_splits: usize,
    },
    /// `CallOnceOptions`.
    CallOnce {
        /// The subgraph to run once, before the first invocation.
        init_subgraph: usize,
    },
    /// `VarHandleOptions`.
    VarHandle {
        /// Name of the shared variable.
        shared_name: Vec<u8>,
    },
}

/// One operator instance.
#[derive(Debug, Clone)]
pub struct Operator {
    /// What it is.
    pub kind: OpKind,
    /// Input tensor indices (-1 for an absent optional input).
    pub inputs: Vec<i32>,
    /// Output tensor indices.
    pub outputs: Vec<i32>,
    /// Its options.
    pub options: Options,
}

/// One subgraph.
#[derive(Debug, Clone, Default)]
pub struct Subgraph {
    /// Tensors.
    pub tensors: Vec<Tensor>,
    /// Input tensor indices.
    pub inputs: Vec<usize>,
    /// Output tensor indices.
    pub outputs: Vec<usize>,
    /// Operators in execution order.
    pub operators: Vec<Operator>,
}

/// A parsed model and the file bytes that back its constant data.
#[derive(Debug, Clone)]
pub struct Model {
    bytes: Vec<u8>,
    /// Subgraph 0 is the main one.
    pub subgraphs: Vec<Subgraph>,
}

fn op_kind(code: i32) -> Result<OpKind> {
    Ok(match code {
        3 => OpKind::Conv2d,
        4 => OpKind::DepthwiseConv2d,
        9 => OpKind::FullyConnected,
        2 => OpKind::Concatenation,
        45 => OpKind::StridedSlice,
        102 => OpKind::SplitV,
        22 => OpKind::Reshape,
        14 => OpKind::Logistic,
        114 => OpKind::Quantize,
        129 => OpKind::CallOnce,
        142 => OpKind::VarHandle,
        143 => OpKind::ReadVariable,
        144 => OpKind::AssignVariable,
        other => {
            return Err(Error::Unsupported(format!(
                "operator {} (builtin code {other})",
                builtin_name(other)
            )));
        }
    })
}

/// Names of a few common operators, to make a refusal readable.
fn builtin_name(code: i32) -> &'static str {
    match code {
        0 => "ADD",
        1 => "AVERAGE_POOL_2D",
        5 => "DEPTH_TO_SPACE",
        6 => "DEQUANTIZE",
        17 => "MAX_POOL_2D",
        18 => "MUL",
        19 => "RELU",
        25 => "SOFTMAX",
        28 => "TANH",
        32 => "CUSTOM",
        34 => "PAD",
        40 => "MEAN",
        41 => "SUB",
        43 => "SQUEEZE",
        _ => "(not in the supported set)",
    }
}

fn small(v: i32, what: &'static str) -> Result<usize> {
    if (1..=4096).contains(&v) {
        Ok(v as usize)
    } else {
        Err(Error::Invalid(format!("{what} {v} is out of range")))
    }
}

fn activation(code: i8, what: &str) -> Result<Activation> {
    match code {
        0 => Ok(Activation::None),
        1 => Ok(Activation::Relu),
        o => Err(Error::Unsupported(format!(
            "fused activation {o} on {what}"
        ))),
    }
}

fn read_options(kind: OpKind, op: &Table<'_>) -> Result<Options> {
    let opts = op.table(4)?;
    // A missing options table means "all defaults", which several of these operators do not allow.
    let need = |what: &'static str| opts.ok_or(Error::Invalid(format!("{what} has no options")));
    Ok(match kind {
        OpKind::Conv2d => {
            let t = need("CONV_2D")?;
            Options::Conv {
                padding: padding(t.i8(0, 0)?)?,
                stride_w: small(t.i32(1, 1)?, "stride_w")?,
                stride_h: small(t.i32(2, 1)?, "stride_h")?,
                activation: activation(t.i8(3, 0)?, "CONV_2D")?,
                dilation_w: small(t.i32(4, 1)?, "dilation_w")?,
                dilation_h: small(t.i32(5, 1)?, "dilation_h")?,
                depth_multiplier: 1,
            }
        }
        OpKind::DepthwiseConv2d => {
            let t = need("DEPTHWISE_CONV_2D")?;
            Options::Conv {
                padding: padding(t.i8(0, 0)?)?,
                stride_w: small(t.i32(1, 1)?, "stride_w")?,
                stride_h: small(t.i32(2, 1)?, "stride_h")?,
                depth_multiplier: small(t.i32(3, 1)?, "depth_multiplier")?,
                activation: activation(t.i8(4, 0)?, "DEPTHWISE_CONV_2D")?,
                dilation_w: small(t.i32(5, 1)?, "dilation_w")?,
                dilation_h: small(t.i32(6, 1)?, "dilation_h")?,
            }
        }
        OpKind::FullyConnected => {
            let t = need("FULLY_CONNECTED")?;
            if t.i8(1, 0)? != 0 {
                return Err(Error::Unsupported(
                    "FULLY_CONNECTED shuffled weights".into(),
                ));
            }
            if t.bool(3, false)? {
                return Err(Error::Unsupported(
                    "FULLY_CONNECTED asymmetric input quantisation".into(),
                ));
            }
            Options::FullyConnected {
                activation: activation(t.i8(0, 0)?, "FULLY_CONNECTED")?,
            }
        }
        OpKind::Concatenation => {
            let t = need("CONCATENATION")?;
            Options::Concat {
                axis: t.i32(0, 0)?,
                activation: activation(t.i8(1, 0)?, "CONCATENATION")?,
            }
        }
        OpKind::StridedSlice => {
            let t = need("STRIDED_SLICE")?;
            if t.bool(5, false)? {
                return Err(Error::Unsupported("STRIDED_SLICE offset mode".into()));
            }
            Options::StridedSlice {
                begin_mask: t.i32(0, 0)?,
                end_mask: t.i32(1, 0)?,
                ellipsis_mask: t.i32(2, 0)?,
                new_axis_mask: t.i32(3, 0)?,
                shrink_axis_mask: t.i32(4, 0)?,
            }
        }
        OpKind::SplitV => {
            let t = need("SPLIT_V")?;
            Options::SplitV {
                num_splits: small(t.i32(0, 0)?, "num_splits")?,
            }
        }
        OpKind::CallOnce => {
            let t = need("CALL_ONCE")?;
            let i = t.i32(0, 0)?;
            if i < 0 {
                return Err(Error::Invalid(
                    "CALL_ONCE subgraph index is negative".into(),
                ));
            }
            Options::CallOnce {
                init_subgraph: i as usize,
            }
        }
        OpKind::VarHandle => {
            let t = need("VAR_HANDLE")?;
            Options::VarHandle {
                shared_name: t.string(1)?.unwrap_or_default().to_vec(),
            }
        }
        OpKind::Reshape
        | OpKind::Logistic
        | OpKind::Quantize
        | OpKind::ReadVariable
        | OpKind::AssignVariable => Options::None,
    })
}

fn padding(code: i8) -> Result<Padding> {
    match code {
        0 => Ok(Padding::Same),
        1 => Ok(Padding::Valid),
        o => Err(Error::Unsupported(format!("padding scheme {o}"))),
    }
}

fn i32_vec(v: Option<crate::flat::Vector<'_>>, what: &'static str) -> Result<Vec<i32>> {
    let Some(v) = v else { return Ok(Vec::new()) };
    if v.len() > MAX_ITEMS {
        return Err(Error::Limit(what));
    }
    (0..v.len()).map(|i| v.i32(i)).collect()
}

fn read_tensor(t: &Table<'_>, buffers: &[Option<(usize, usize)>]) -> Result<Tensor> {
    let dims = i32_vec(t.vector(0)?, "tensor rank")?;
    if dims.len() > MAX_RANK {
        return Err(Error::Limit("tensor rank above 4"));
    }
    let mut shape = Vec::with_capacity(dims.len());
    let mut elements: usize = 1;
    for d in dims {
        if d < 0 {
            return Err(Error::Unsupported("dynamic tensor shape".into()));
        }
        let d = d as usize;
        elements = elements
            .checked_mul(d)
            .filter(|&e| e <= MAX_ELEMENTS)
            .ok_or(Error::Limit("tensor with more than 1,048,576 elements"))?;
        shape.push(d);
    }
    let ty = TensorType::from_code(t.i8(1, 0)?);
    let buffer = t.i32(2, 0)?;
    let data = if buffer > 0 {
        buffers
            .get(buffer as usize)
            .copied()
            .ok_or(Error::Malformed("tensor buffer index out of range"))?
    } else {
        None
    };
    if let (Some((_, len)), Some(sz)) = (data, ty.size()) {
        if len != elements * sz {
            return Err(Error::Invalid(format!(
                "constant data of {len} bytes does not match a tensor of {elements} elements"
            )));
        }
    }
    let mut quant = Quant::default();
    if let Some(q) = t.table(4)? {
        if let Some(s) = q.vector(2)? {
            if s.len() > MAX_ITEMS {
                return Err(Error::Limit("quantisation channels"));
            }
            quant.scale = (0..s.len()).map(|i| s.f32(i)).collect::<Result<_>>()?;
        }
        if let Some(z) = q.vector(3)? {
            if z.len() > MAX_ITEMS {
                return Err(Error::Limit("quantisation channels"));
            }
            quant.zero_point = (0..z.len()).map(|i| z.i64(i)).collect::<Result<_>>()?;
        }
        let dim = q.i32(6, 0)?;
        quant.dim = usize::try_from(dim)
            .map_err(|_| Error::Invalid("negative quantised dimension".into()))?;
        if !quant.zero_point.is_empty() && quant.zero_point.len() != quant.scale.len() {
            return Err(Error::Invalid("scale and zero point counts differ".into()));
        }
        if quant.scale.iter().any(|s| !s.is_finite() || *s <= 0.0) {
            return Err(Error::Invalid(
                "quantisation scale is not a positive number".into(),
            ));
        }
    }
    Ok(Tensor {
        shape,
        ty,
        quant,
        data,
        is_variable: t.bool(5, false)?,
    })
}

impl Model {
    /// Parse and structurally validate a model file. Takes ownership of the bytes so constant data can stay in
    /// place.
    pub fn parse(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(Error::Limit("model file over 1 MiB"));
        }
        let subgraphs = {
            let root = Buf::new(&bytes).root()?;
            // The file identifier "TFL3" sits at bytes 4..8 of a real model; the schema version is field 0.
            if bytes.get(4..8) != Some(b"TFL3") {
                return Err(Error::Malformed(
                    "not a TFLite model (missing TFL3 identifier)",
                ));
            }
            if root.i32(0, 0)? != 3 {
                return Err(Error::Unsupported(
                    "TFLite schema version other than 3".into(),
                ));
            }
            // Buffers: byte ranges of constant data.
            let mut buffers: Vec<Option<(usize, usize)>> = Vec::new();
            if let Some(bv) = root.vector(4)? {
                if bv.len() > MAX_ITEMS {
                    return Err(Error::Limit("more than 1,024 buffers"));
                }
                for i in 0..bv.len() {
                    let b = bv.table(i)?;
                    buffers.push(match b.vector(0)? {
                        Some(d) if !d.is_empty() => Some(d.byte_range(1)?),
                        _ => None,
                    });
                }
            }
            // Operator codes.
            let mut codes = Vec::new();
            if let Some(cv) = root.vector(1)? {
                if cv.len() > MAX_ITEMS {
                    return Err(Error::Limit("more than 1,024 operator codes"));
                }
                for i in 0..cv.len() {
                    let c = cv.table(i)?;
                    let deprecated = i32::from(c.i8(0, 0)?);
                    let builtin = c.i32(3, 0)?;
                    codes.push(deprecated.max(builtin));
                }
            }
            let sv = root
                .vector(2)?
                .ok_or(Error::Malformed("the model has no subgraphs"))?;
            if sv.is_empty() || sv.len() > MAX_SUBGRAPHS {
                return Err(Error::Limit("subgraph count"));
            }
            let mut subgraphs = Vec::with_capacity(sv.len());
            for si in 0..sv.len() {
                subgraphs.push(read_subgraph(&sv.table(si)?, &buffers, &codes)?);
            }
            subgraphs
        };
        Ok(Self { bytes, subgraphs })
    }

    /// The constant data of a tensor, if it has any.
    #[must_use]
    pub fn data(&self, t: &Tensor) -> Option<&[u8]> {
        let (start, len) = t.data?;
        self.bytes.get(start..start.checked_add(len)?)
    }

    /// The whole file.
    #[must_use]
    pub fn file(&self) -> &[u8] {
        &self.bytes
    }
}

fn read_subgraph(
    g: &Table<'_>,
    buffers: &[Option<(usize, usize)>],
    codes: &[i32],
) -> Result<Subgraph> {
    let mut sg = Subgraph::default();
    if let Some(tv) = g.vector(0)? {
        if tv.len() > MAX_ITEMS {
            return Err(Error::Limit("more than 1,024 tensors"));
        }
        for i in 0..tv.len() {
            sg.tensors.push(read_tensor(&tv.table(i)?, buffers)?);
        }
    }
    let n = sg.tensors.len();
    let index = |v: i32| -> Result<usize> {
        usize::try_from(v)
            .ok()
            .filter(|&u| u < n)
            .ok_or(Error::Malformed("tensor index out of range"))
    };
    sg.inputs = i32_vec(g.vector(1)?, "subgraph inputs")?
        .into_iter()
        .map(index)
        .collect::<Result<_>>()?;
    sg.outputs = i32_vec(g.vector(2)?, "subgraph outputs")?
        .into_iter()
        .map(index)
        .collect::<Result<_>>()?;
    if let Some(ov) = g.vector(3)? {
        if ov.len() > MAX_ITEMS {
            return Err(Error::Limit("more than 1,024 operators"));
        }
        for i in 0..ov.len() {
            let op = ov.table(i)?;
            let code_index = op.i32(0, 0)?;
            let code = usize::try_from(code_index)
                .ok()
                .and_then(|c| codes.get(c).copied())
                .ok_or(Error::Malformed("operator code index out of range"))?;
            let kind = op_kind(code)?;
            let inputs = i32_vec(op.vector(1)?, "operator inputs")?;
            let outputs = i32_vec(op.vector(2)?, "operator outputs")?;
            for &t in inputs.iter().chain(outputs.iter()) {
                if t != -1 {
                    index(t)?;
                }
            }
            if outputs.contains(&-1) {
                return Err(Error::Malformed("operator output index -1"));
            }
            let options = read_options(kind, &op)?;
            sg.operators.push(Operator {
                kind,
                inputs,
                outputs,
                options,
            });
        }
    }
    Ok(sg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_wak_002_bytes_that_are_not_a_model_are_refused() {
        assert!(Model::parse(Vec::new()).is_err());
        assert!(Model::parse(vec![0; 64]).is_err());
        // A plausible root offset but no TFL3 identifier.
        let mut b = vec![0u8; 64];
        b[0] = 16;
        assert!(matches!(Model::parse(b), Err(Error::Malformed(_))));
    }

    #[test]
    fn req_wak_002_a_file_over_the_limit_is_refused_before_it_is_read() {
        assert!(matches!(
            Model::parse(vec![0; MAX_FILE_BYTES + 1]),
            Err(Error::Limit(_))
        ));
    }

    #[test]
    fn req_wak_001_unsupported_operators_are_named() {
        let e = op_kind(0).unwrap_err();
        assert!(e.to_string().contains("ADD"), "{e}");
        assert!(op_kind(3).is_ok() && op_kind(142).is_ok());
        assert!(op_kind(9999).is_err());
    }

    #[test]
    fn tensor_types_have_sizes() {
        assert_eq!(TensorType::from_code(9).size(), Some(1));
        assert_eq!(TensorType::from_code(2).size(), Some(4));
        assert_eq!(TensorType::from_code(13).size(), None);
    }
}
