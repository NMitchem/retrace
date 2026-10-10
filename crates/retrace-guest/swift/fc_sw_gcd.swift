// M49 first-contact corpus (spec §3b): DispatchQueue.async onto a serial queue, four times, then a
// DispatchGroup wait. The queue's work runs on a libdispatch worker thread (M18's workqueue), and
// the serial queue keeps the sum's order fixed.
import Dispatch

final class Total: @unchecked Sendable { var value = 0 }

let total = Total()
let queue = DispatchQueue(label: "firstcontact")
let group = DispatchGroup()
for i in 1...4 {
    queue.async(group: group) { total.value += i * i }
}
group.wait()
print("sum of squares=\(total.value)")
