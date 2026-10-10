// M49 first-contact corpus (spec §3b), row S: Foundation's Thread.sleep(forTimeInterval: 0.05),
// which calls nanosleep(&ts, NULL), straight to __semwait_signal (334) (spec §2e). Prints whether
// DispatchTime (mach_absolute_time) advanced by at least 50 ms.
import Foundation

print("sleeping 50 ms")
let t0 = DispatchTime.now().uptimeNanoseconds
Thread.sleep(forTimeInterval: 0.05)
let dt = DispatchTime.now().uptimeNanoseconds - t0
print("ok=\(dt >= 50_000_000 ? 1 : 0)")
