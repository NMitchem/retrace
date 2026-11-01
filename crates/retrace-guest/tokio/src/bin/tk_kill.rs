// current-thread runtime; a plain std thread sleeps briefly, then kill(getpid(), SIGUSR1) while main is in kevent
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut s = signal(SignalKind::user_defined1()).unwrap();
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
    });
    s.recv().await;
    println!("tokio kill: got SIGUSR1");
}
