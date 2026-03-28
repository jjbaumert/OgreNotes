use aws_sdk_s3::Client;
use aws_sdk_s3::presigning::PresigningConfig;
use std::time::Duration;

/// Wrapper around the S3 client for blob and snapshot operations.
#[derive(Clone)]
pub struct S3Client {
    client: Client,
    bucket: String,
}

impl S3Client {
    pub fn new(client: Client, bucket: String) -> Self {
        Self { client, bucket }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Generate a presigned PUT URL for client-side upload.
    pub async fn presigned_put_url(
        &self,
        key: &str,
        content_type: &str,
        ttl_secs: u64,
    ) -> Result<String, S3Error> {
        let config = PresigningConfig::builder()
            .expires_in(Duration::from_secs(ttl_secs))
            .build()
            .map_err(|e| S3Error::Presign(e.to_string()))?;

        let presigned = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .presigned(config)
            .await
            .map_err(|e| S3Error::Presign(e.to_string()))?;

        Ok(presigned.uri().to_string())
    }

    /// Generate a presigned GET URL for client-side download.
    pub async fn presigned_get_url(
        &self,
        key: &str,
        ttl_secs: u64,
    ) -> Result<String, S3Error> {
        let config = PresigningConfig::builder()
            .expires_in(Duration::from_secs(ttl_secs))
            .build()
            .map_err(|e| S3Error::Presign(e.to_string()))?;

        let presigned = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(config)
            .await
            .map_err(|e| S3Error::Presign(e.to_string()))?;

        Ok(presigned.uri().to_string())
    }

    /// Upload bytes directly from the server (for snapshots).
    pub async fn put_object(&self, key: &str, data: Vec<u8>) -> Result<(), S3Error> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(data.into())
            .send()
            .await
            .map_err(|e| S3Error::Operation(e.into_service_error().to_string()))?;

        Ok(())
    }

    /// Download bytes directly on the server (for snapshots).
    pub async fn get_object(&self, key: &str) -> Result<Vec<u8>, S3Error> {
        let result = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| S3Error::Operation(e.into_service_error().to_string()))?;

        let bytes = result
            .body
            .collect()
            .await
            .map_err(|e| S3Error::Operation(e.to_string()))?;

        Ok(bytes.to_vec())
    }

    /// Check if an object exists.
    pub async fn object_exists(&self, key: &str) -> Result<bool, S3Error> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => {
                let service_err = e.into_service_error();
                if service_err.is_not_found() {
                    Ok(false)
                } else {
                    Err(S3Error::Operation(service_err.to_string()))
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    #[error("presigning error: {0}")]
    Presign(String),

    #[error("S3 operation error: {0}")]
    Operation(String),
}
