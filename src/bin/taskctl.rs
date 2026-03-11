#[tokio::main]
async fn main() {
    taskd::exit(taskd::run_taskctl().await);
}
