"""List operators, tensor shapes and sizes of a .tflite file (needs `pip install tflite`)."""
import sys, collections
import tflite

buf = open(sys.argv[1], "rb").read()
m = tflite.Model.GetRootAsModel(buf, 0)
g = m.Subgraphs(0)
names = {v: k for k, v in vars(tflite.BuiltinOperator).items() if not k.startswith("_")}
ops = collections.Counter()
for i in range(g.OperatorsLength()):
    code = m.OperatorCodes(g.Operators(i).OpcodeIndex())
    ops[names.get(max(code.BuiltinCode(), code.DeprecatedBuiltinCode()), "?")] += 1
print("file bytes", len(buf), "version", m.Version())
print("ops", dict(ops))
nbytes = 0
for i in range(g.TensorsLength()):
    t = g.Tensors(i)
    b = m.Buffers(t.Buffer()).DataLength()
    nbytes += b
print("tensors", g.TensorsLength(), "constant data bytes", nbytes)
for i in list(g.InputsAsNumpy()) + list(g.OutputsAsNumpy()):
    t = g.Tensors(i)
    print("io", t.Name(), list(t.ShapeAsNumpy()), t.Type())

# Multiply-accumulates per inference (one inference per 30 ms of audio): weights are inputs[1], output is outputs[0].
macs = 0
for i in range(g.OperatorsLength()):
    op = g.Operators(i)
    code = m.OperatorCodes(op.OpcodeIndex())
    name = names.get(max(code.BuiltinCode(), code.DeprecatedBuiltinCode()), "?")
    if name in ("CONV_2D", "DEPTHWISE_CONV_2D", "FULLY_CONNECTED"):
        w = [int(x) for x in g.Tensors(op.Inputs(1)).ShapeAsNumpy()]
        n_out = 1
        for d in g.Tensors(op.Outputs(0)).ShapeAsNumpy():
            n_out *= int(d)
        if name == "CONV_2D":
            macs += n_out * w[1] * w[2] * w[3]
        elif name == "DEPTHWISE_CONV_2D":
            macs += n_out * w[1] * w[2]
        else:
            macs += n_out * w[1]
print("MACs per inference", macs, "->", round(macs * 100 / 3 / 1e6, 2), "MMAC/s at 33.3 inferences/s")
