// M14's headline guest. A stock full-std Rust binary with two threads of control.
//
// `joined 42` is the load-bearing line: it can be printed only if the child thread genuinely ran
// AND its return value crossed back through join. Exit 0 proves nothing — a guest that never
// spawned also exits 0, which is the trap segv_rust_e2e documented and protnone_rust_e2e sharpened.
fn main() {
    println!("main before spawn");
    let h = std::thread::spawn(|| {
        println!("child ran");
        42u32
    });
    let v = h.join().unwrap();
    println!("joined {v}");
}
