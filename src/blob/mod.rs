pub mod s3;
pub mod gcs;
pub mod azure;

use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;

#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn upload(&self, local_path: &Path, remote_key: &str) -> Result<String>;
}

pub enum StoreConfig {
    S3 {
        bucket: String,
        region: String,
    },
    Gcs {
        bucket: String,
        credentials_path: Option<String>,
        access_token: Option<String>,
    },
    Azure {
        account: String,
        container: String,
        access_key: String,
    },
}

impl StoreConfig {
    pub fn from_env() -> Result<Self> {
        let provider = std::env::var("BLOB_PROVIDER")
            .context("BLOB_PROVIDER not set (s3|gcs|azure)")?;

        match provider.as_str() {
            "s3" => Ok(Self::S3 {
                bucket: std::env::var("S3_BUCKET").context("S3_BUCKET not set")?,
                region: std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into()),
            }),
            "gcs" => Ok(Self::Gcs {
                bucket: std::env::var("GCS_BUCKET").context("GCS_BUCKET not set")?,
                credentials_path: std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok(),
                access_token: std::env::var("GCS_ACCESS_TOKEN").ok(),
            }),
            "azure" => Ok(Self::Azure {
                account: std::env::var("AZURE_STORAGE_ACCOUNT")
                    .context("AZURE_STORAGE_ACCOUNT not set")?,
                container: std::env::var("AZURE_STORAGE_CONTAINER")
                    .context("AZURE_STORAGE_CONTAINER not set")?,
                access_key: std::env::var("AZURE_STORAGE_ACCESS_KEY")
                    .context("AZURE_STORAGE_ACCESS_KEY not set")?,
            }),
            other => anyhow::bail!("unknown BLOB_PROVIDER: {other}"),
        }
    }

    pub async fn build(self) -> Result<Box<dyn BlobStore>> {
        match self {
            Self::S3 { bucket, region } => {
                Ok(Box::new(s3::S3Store::new(&bucket, &region).await))
            }
            Self::Gcs {
                bucket,
                credentials_path,
                access_token,
            } => Ok(Box::new(
                gcs::GcsStore::new(&bucket, credentials_path, access_token).await?,
            )),
            Self::Azure {
                account,
                container,
                access_key,
            } => Ok(Box::new(azure::AzureStore::new(
                &account,
                &container,
                &access_key,
            ))),
        }
    }
}
