// the same on the multi-thread runtime: main is parked in a condition variable while a worker drives the driver
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() {
    let mut s = signal(SignalKind::user_defined1()).unwrap();
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(50));
        unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
    });
    s.recv().await;
    println!("tokio kill_mt: got SIGUSR1");
}
