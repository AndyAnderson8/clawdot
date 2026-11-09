"""Dump the full graph of a .tflite file: every operator with its options, tensor shapes, types and quantisation.

Usage: python -I dump_graph.py MODEL.tflite    (needs `pip install tflite`)
Used to design the operator set of crates/wake (R-11 section 8).
"""
import sys
import tflite

buf = open(sys.argv[1], "rb").read()
m = tflite.Model.GetRootAsModel(buf, 0)
ops_names = {v: k for k, v in vars(tflite.BuiltinOperator).items() if not k.startswith("_")}
types = {v: k for k, v in vars(tflite.TensorType).items() if not k.startswith("_")}
print("subgraphs", m.SubgraphsLength(), "buffers", m.BuffersLength(), "metadata", m.MetadataLength())
for si in range(m.SubgraphsLength()):
    g = m.Subgraphs(si)
    print(f"== subgraph {si} name={g.Name()} inputs={list(g.InputsAsNumpy())} outputs={list(g.OutputsAsNumpy())}")
    for i in range(g.TensorsLength()):
        t = g.Tensors(i)
        q = t.Quantization()
        qs = ""
        if q is not None and q.ScaleLength():
            qs = f" scale={[float(x) for x in q.ScaleAsNumpy()][:3]}{'...' if q.ScaleLength()>3 else ''}(n={q.ScaleLength()}) zp={[int(x) for x in q.ZeroPointAsNumpy()][:3]} qdim={q.QuantizedDimension()}"
        shape = [int(x) for x in t.ShapeAsNumpy()] if t.ShapeLength() else []
        print(f"  t{i} {t.Name().decode()} {types[t.Type()]} {shape} buf={t.Buffer()} var={t.IsVariable()}{qs}")
    for i in range(g.OperatorsLength()):
        op = g.Operators(i)
        code = m.OperatorCodes(op.OpcodeIndex())
        name = ops_names.get(max(code.BuiltinCode(), code.DeprecatedBuiltinCode()), "?")
        extra = ""
        bo = op.BuiltinOptionsType()
        if bo:
            tab = op.BuiltinOptions()
            if tab is not None:
                cls = None
                for n, v in vars(tflite.BuiltinOptions).items():
                    if v == bo:
                        cls = getattr(tflite, n, None) or getattr(__import__("tflite." + n, fromlist=[n]), n)
                o = cls()
                o.Init(tab.Bytes, tab.Pos)
                fields = {a: getattr(o, a)() for a in dir(o) if a[0].isupper() and not a.startswith("GetRoot") and not a.endswith(("AsNumpy", "Length", "IsNone")) and not a.startswith("Init") and "Start" not in a and "BufferHasIdentifier" not in a and "GetRoot" not in a}
                extra = " " + str(fields)
        print(f"  op{i} {name} in={list(op.InputsAsNumpy())} out={list(op.OutputsAsNumpy())}{extra}")
    if si > 0:
        pass
