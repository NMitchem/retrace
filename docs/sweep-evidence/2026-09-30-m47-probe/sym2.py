import lldb
def __lldb_init_module(debugger, d):
    t = debugger.GetSelectedTarget(); p = t.GetProcess()
    m = t.FindModule(lldb.SBFileSpec("libsystem_c.dylib"))
    h = m.GetObjectFileHeaderAddress()
    slide = h.GetLoadAddress(t) - h.GetFileAddress()
    print("cache slide", hex(slide))
    for a in (0x18019591c, 0x18df15a78):
        sa = t.ResolveLoadAddress(a + slide)
        s = sa.GetSymbol()
        print(hex(a), sa.GetModule().GetFileSpec().GetFilename(), s.GetName(), "+", hex(a + slide - s.GetStartAddress().GetLoadAddress(t)) if s.IsValid() else "")
    for a in (0x1801ab6ee, 0x18df1b45b, 0x1801b7eb4, 0x18df1ac55):
        err = lldb.SBError()
        print(hex(a), repr(p.ReadCStringFromMemory(a + slide, 64, err)), err.Success())
