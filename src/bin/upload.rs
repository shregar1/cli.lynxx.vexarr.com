use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use lynxx_cli::blob::StoreConfig;

#[derive(Parser)]
#[command(about = "Upload a file to the configured blob store")]
struct Args {
    #[arg(long)]
    file: PathBuf,
    #[arg(long)]
    key: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let store = StoreConfig::from_env()?.build().await?;
    let uri = store.upload(&args.file, &args.key).await?;
    println!("{uri}");
    Ok(())
}
