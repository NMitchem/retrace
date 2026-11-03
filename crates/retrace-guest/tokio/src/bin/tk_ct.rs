// the current-thread flavor with `full`
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let h = tokio::spawn(async { 40 + 2 });
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    println!("tokio ct: {}", h.await.unwrap());
}
