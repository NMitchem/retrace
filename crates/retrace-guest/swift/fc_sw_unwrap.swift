// M49 first-contact corpus (spec §3b): a force-unwrap of nil. The Swift runtime traps with `brk`:
// natively SIGTRAP (133), and the trace's terminal event is `Event::Crash`. The optional comes from
// argc so the compiler cannot see it; stdout is flushed before the trap.
import Darwin

let name: String? = CommandLine.arguments.count > 5 ? "five" : nil
print("unwrapping")
fflush(stdout)
print(name!)
