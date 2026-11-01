// select! between ctrl_c and a sleep: the sleep wins
use std::time::Duration;

#[tokio::main]
async fn main() {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => println!("tokio select: ctrl_c"),
        _ = tokio::time::sleep(Duration::from_millis(20)) => println!("tokio select: sleep"),
    }
}
