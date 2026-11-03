// mpsc, spawn_blocking and time
use std::time::Duration;

#[tokio::main]
async fn main() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<u32>(4);
    let producer = tokio::spawn(async move {
        for i in 1..=3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
            tx.send(i * 10).await.unwrap();
        }
    });
    let mut sum = 0;
    while let Some(v) = rx.recv().await {
        sum += v;
    }
    producer.await.unwrap();
    let blocking = tokio::task::spawn_blocking(move || sum + 1).await.unwrap();
    println!("tokio chan: sum={} blocking={}", sum, blocking);
}
