// spawn, sleep, print: the multi-thread runtime with `full`
#[tokio::main]
async fn main() {
    let h = tokio::spawn(async { 40 + 2 });
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    println!("tokio full: {}", h.await.unwrap());
}
