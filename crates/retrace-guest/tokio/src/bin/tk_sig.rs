// raise(SIGUSR1) on the calling thread, then await the signal stream
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut s = signal(SignalKind::user_defined1()).unwrap();
    unsafe { libc::raise(libc::SIGUSR1) };
    s.recv().await;
    println!("tokio sig: got SIGUSR1");
}
