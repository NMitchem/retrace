// tokio::fs reads a fixture file; the path is argv[1], default data/tk_fs.txt
#[tokio::main]
async fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "data/tk_fs.txt".to_string());
    let s = tokio::fs::read_to_string(&path).await.unwrap();
    println!("tokio fs: {} bytes, first line {:?}", s.len(), s.lines().next().unwrap_or(""));
}
