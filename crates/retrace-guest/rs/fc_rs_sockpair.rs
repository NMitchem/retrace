// M55 t4 first-contact row: std's `UnixStream::pair()`, nonblocking, a round trip, then one end dropped.
// The other end reads 0, and a write gives an error of kind `BrokenPipe` (Rust ignores SIGPIPE).
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;

fn main() {
    let (mut a, mut b) = UnixStream::pair().expect("pair");
    a.set_nonblocking(true).expect("nonblocking a");
    b.set_nonblocking(true).expect("nonblocking b");
    let mut buf = [0u8; 16];
    println!("empty read: {:?}", b.read(&mut buf).map_err(|e| e.kind()));
    println!("write: {:?}", a.write(b"ping").map_err(|e| e.kind()));
    let n = b.read(&mut buf).expect("read");
    println!("read {} {}", n, String::from_utf8_lossy(&buf[..n]));
    drop(a);
    println!("read after drop: {:?}", b.read(&mut buf).map_err(|e| e.kind()));
    match b.write(b"late") {
        Ok(n) => println!("write after drop: ok {n}"),
        Err(e) => println!("write after drop: {:?} {}", e.kind(), e.kind() == ErrorKind::BrokenPipe),
    }
}
