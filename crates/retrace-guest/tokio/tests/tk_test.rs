// a #[tokio::test] binary in tokio's own signal_usr1 shape: libtest's main thread waits, the test body runs on a
// worker and kill(getpid())s itself. Run as `<bin> --test-threads=1`.
use tokio::signal::unix::{signal, SignalKind};

#[tokio::test]
async fn signal_usr1() {
    let mut s = signal(SignalKind::user_defined1()).unwrap();
    unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
    s.recv().await;
    println!("tokio test: got SIGUSR1");
}
