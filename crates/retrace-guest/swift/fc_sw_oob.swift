// M49 first-contact corpus (spec §3b): an array index out of range. The Swift runtime traps with
// `brk`, so natively SIGTRAP (133) and the trace's terminal event is `Event::Crash` (a hardware
// exception, CLAUDE.md's raise/fault distinction). The index comes from argc so the compiler cannot
// see it; stdout is flushed before the trap.
import Darwin

let a = [1, 2, 3]
let i = a.count + CommandLine.arguments.count - 1
print("indexing \(i) of \(a.count)")
fflush(stdout)
print(a[i])
