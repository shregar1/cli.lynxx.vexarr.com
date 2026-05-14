use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;

use super::BlobStore;

pub struct S3Store {
    client: Client,
    bucket: String,
}

impl S3Store {
    pub async fn new(bucket: &str, region: &str) -> Self {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()))
            .load()
            .await;
        Self {
            client: Client::new(&config),
            bucket: bucket.to_string(),
        }
    }
}

#[async_trait]
impl BlobStore for S3Store {
    async fn upload(&self, local_path: &Path, remote_key: &str) -> Result<String> {
        let body = ByteStream::from_path(local_path).await?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(remote_key)
            .body(body)
            .send()
            .await?;
        Ok(format!("s3://{}/{}", self.bucket, remote_key))
    }
}
