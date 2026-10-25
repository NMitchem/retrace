// M52 t1 (spec §3b): a Rust Vec grown past 2 MiB into an in-place huge chunk, to 24 MiB (an xzone huge chunk, grown by realloc),
// then a thread spawned (libstd mmaps the new thread's sigaltstack with mmap(NULL, ..)). Before the
// bump cursor floated past FIXED placements this was the bug banner. Adopted from the t0 vecgrow.rs.
fn main() {
    let mut v: Vec<u8> = Vec::new();
    for i in 0..(24usize << 20) { v.push(i as u8); }
    let h = std::thread::spawn(|| 7u32);
    assert_eq!(h.join().unwrap(), 7);
    assert_eq!(v.len(), 24 << 20);
    println!("vecgrow ok");
}
