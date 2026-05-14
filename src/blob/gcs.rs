use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use super::BlobStore;

pub struct GcsStore {
    client: Client,
    bucket: String,
    access_token: String,
}

#[derive(Deserialize)]
struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    token_uri: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

impl GcsStore {
    pub async fn new(
        bucket: &str,
        credentials_path: Option<String>,
        access_token: Option<String>,
    ) -> Result<Self> {
        let token = match access_token {
            Some(t) => t,
            None => {
                let creds_path = credentials_path
                    .context("set GCS_ACCESS_TOKEN or GOOGLE_APPLICATION_CREDENTIALS")?;
                Self::token_from_service_account(&creds_path).await?
            }
        };
        Ok(Self {
            client: Client::new(),
            bucket: bucket.to_string(),
            access_token: token,
        })
    }

    async fn token_from_service_account(path: &str) -> Result<String> {
        let key_json = std::fs::read_to_string(path)
            .with_context(|| format!("reading service account key from {path}"))?;
        let key: ServiceAccountKey = serde_json::from_str(&key_json)?;

        let now = chrono::Utc::now().timestamp();
        let claims = serde_json::json!({
            "iss": key.client_email,
            "scope": "https://www.googleapis.com/auth/devstorage.read_write",
            "aud": key.token_uri,
            "iat": now,
            "exp": now + 3600,
        });

        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let encoding_key =
            jsonwebtoken::EncodingKey::from_rsa_pem(key.private_key.as_bytes())?;
        let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)?;

        let resp: TokenResponse = Client::new()
            .post(&key.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(resp.access_token)
    }
}

#[async_trait]
impl BlobStore for GcsStore {
    async fn upload(&self, local_path: &Path, remote_key: &str) -> Result<String> {
        let data = tokio::fs::read(local_path).await?;
        let url = format!(
            "https://storage.googleapis.com/upload/storage/v1/b/{}/o",
            self.bucket,
        );

        self.client
            .post(&url)
            .query(&[("uploadType", "media"), ("name", remote_key)])
            .bearer_auth(&self.access_token)
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .send()
            .await?
            .error_for_status()?;

        Ok(format!("gs://{}/{}", self.bucket, remote_key))
    }
}
