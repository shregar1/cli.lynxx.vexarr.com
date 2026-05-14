use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::Sha256;

use super::BlobStore;

pub struct AzureStore {
    client: Client,
    account: String,
    container: String,
    access_key: Vec<u8>,
}

impl AzureStore {
    pub fn new(account: &str, container: &str, access_key_b64: &str) -> Self {
        Self {
            client: Client::new(),
            account: account.to_string(),
            container: container.to_string(),
            access_key: STANDARD
                .decode(access_key_b64)
                .expect("AZURE_STORAGE_ACCESS_KEY is not valid base64"),
        }
    }

    fn sign(&self, content_length: usize, date: &str, blob_path: &str) -> String {
        let string_to_sign = format!(
            "PUT\n\n\n{}\n\napplication/octet-stream\n\n\n\n\n\n\n\
             x-ms-blob-type:BlockBlob\nx-ms-date:{}\nx-ms-version:2020-10-02\n\
             /{}/{}",
            content_length, date, self.account, blob_path
        );

        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.access_key).expect("HMAC key error");
        mac.update(string_to_sign.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());

        format!("SharedKey {}:{}", self.account, signature)
    }
}

#[async_trait]
impl BlobStore for AzureStore {
    async fn upload(&self, local_path: &Path, remote_key: &str) -> Result<String> {
        let data = tokio::fs::read(local_path).await?;
        let date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let blob_path = format!("{}/{}", self.container, remote_key);
        let url = format!(
            "https://{}.blob.core.windows.net/{}",
            self.account, blob_path
        );

        let auth = self.sign(data.len(), &date, &blob_path);

        self.client
            .put(&url)
            .header("Authorization", auth)
            .header("x-ms-date", &date)
            .header("x-ms-version", "2020-10-02")
            .header("x-ms-blob-type", "BlockBlob")
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .send()
            .await?
            .error_for_status()?;

        Ok(format!(
            "https://{}.blob.core.windows.net/{}/{}",
            self.account, self.container, remote_key
        ))
    }
}
