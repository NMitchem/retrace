// M49 first-contact corpus (spec §3b): async/await with a task group, top-level await in the main
// file. The Swift concurrency runtime runs the child tasks on its cooperative pool; the sum does not
// depend on their order.
func square(_ x: Int) async -> Int { x * x }

let total = await withTaskGroup(of: Int.self) { group in
    for i in 1...4 { group.addTask { await square(i) } }
    var sum = 0
    for await v in group { sum += v }
    return sum
}
print("sum of squares=\(total)")
