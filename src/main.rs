#[tokio::main]
async fn main() {
    taskd::exit(taskd::run_taskd().await);
}
