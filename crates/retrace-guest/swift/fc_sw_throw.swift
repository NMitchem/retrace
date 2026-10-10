// M49 first-contact corpus (spec §3b): a Swift error thrown and caught. Swift errors are ordinary
// returns, not unwinding, so this exercises the runtime's existential boxing rather than libunwind.
enum Failure: Error { case boom(Int) }

func risky(_ n: Int) throws -> Int {
    if n > 1 { throw Failure.boom(n) }
    return n
}

do {
    _ = try risky(3)
    print("not thrown")
} catch Failure.boom(let n) {
    print("caught boom \(n)")
}
