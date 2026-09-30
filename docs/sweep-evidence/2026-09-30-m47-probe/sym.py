import lldb
def __lldb_init_module(debugger, d):
    t = debugger.GetSelectedTarget()
    m = t.FindModule(lldb.SBFileSpec("libsystem_c.dylib"))
    h = m.GetObjectFileHeaderAddress()
    slide = h.GetLoadAddress(t) - h.GetFileAddress()
    print("cache slide", hex(slide))
    for line in open("frames.txt"):
        a = int(line, 16)
        if a >= 0x180000000:
            sa = t.ResolveLoadAddress(a + slide)
            print(hex(a), sa.GetModule().GetFileSpec().GetFilename(), sa.GetSymbol().GetName(), "+", hex(a + slide - sa.GetSymbol().GetStartAddress().GetLoadAddress(t)) if sa.GetSymbol().IsValid() else "")
