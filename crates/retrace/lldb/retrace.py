"""retrace's lldb commands (M43). Load with `command script import <repo>/crates/retrace/lldb/retrace.py`.

rsi: reverse step one instruction. lldb-2100 has no reverse step of its own (it never sends `bs`),
so this arms the server (`monitor arm-rsi`) and continues in reverse; the server answers that one
`bc` with a single step back. Like `process continue -R`, it leaves lldb's direction reversed: a
plain `continue` afterwards also goes BACKWARD until `process continue -F`. If the reverse resume
fails, the server is still armed from the `arm-rsi` above; `rsi` disarms it (`monitor disarm-rsi`)
before reporting the error, so the next `process continue -R` is a reverse continue again, not one
more step back (M44 B2).
"""
import lldb


def rsi(debugger, command, result, internal_dict):
    ci = debugger.GetCommandInterpreter()
    r = lldb.SBCommandReturnObject()
    ci.HandleCommand("process plugin packet monitor arm-rsi", r)
    if not r.Succeeded():
        result.SetError("retrace rsi: arming the server failed: " + (r.GetError() or ""))
        return
    process = debugger.GetSelectedTarget().GetProcess()
    err = process.ContinueInDirection(lldb.eRunReverse)
    if not err.Success():
        # M44 B2: the server is still armed. Disarm it, or the next `process continue -R` is one
        # instruction back instead of a reverse continue.
        ci.HandleCommand("process plugin packet monitor disarm-rsi", lldb.SBCommandReturnObject())
        result.SetError("retrace rsi: " + str(err))
        return
    ci.HandleCommand("process status", result)


def __lldb_init_module(debugger, internal_dict):
    debugger.HandleCommand("command script add -f retrace.rsi rsi")
