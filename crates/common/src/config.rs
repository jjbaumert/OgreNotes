use std::env;
use std::fmt;

/// Application configuration loaded from environment variables.
#[derive(Clone)]
pub struct AppConfig {
    // AWS
    pub aws_region: String,
    pub dynamodb_table_prefix: String,
    pub s3_bucket: String,

    // Redis
    pub redis_url: String,

    // Auth
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
    pub oauth_redirect_uri: String,
    pub jwt_secret: String,

    // Server
    pub api_port: u16,
    pub frontend_origin: String,

    /// Enable dev-only features (dev-login endpoint). MUST be false in production.
    pub dev_mode: bool,
}

/// Manual Debug implementation that redacts secrets.
impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("aws_region", &self.aws_region)
            .field("dynamodb_table_prefix", &self.dynamodb_table_prefix)
            .field("s3_bucket", &self.s3_bucket)
            .field("redis_url", &"[redacted]")
            .field("oauth_client_id", &self.oauth_client_id)
            .field("oauth_client_secret", &"[redacted]")
            .field("oauth_redirect_uri", &self.oauth_redirect_uri)
            .field("jwt_secret", &"[redacted]")
            .field("api_port", &self.api_port)
            .field("frontend_origin", &self.frontend_origin)
            .finish()
    }
}

impl AppConfig {
    /// Load configuration from environment variables.
    /// Panics if required variables are missing or invalid.
    pub fn from_env() -> Self {
        let api_port_str = env_or("API_PORT", "3000");
        let api_port: u16 = api_port_str
            .parse()
            .unwrap_or_else(|_| panic!("API_PORT must be a valid port number (0-65535), got: {api_port_str}"));

        Self {
            aws_region: env_or("AWS_REGION", "us-east-1"),
            dynamodb_table_prefix: env_required("DYNAMODB_TABLE_PREFIX"),
            s3_bucket: env_required("S3_BUCKET"),
            redis_url: env_or("REDIS_URL", "redis://localhost:6379"),
            oauth_client_id: env_required("OAUTH_CLIENT_ID"),
            oauth_client_secret: env_required("OAUTH_CLIENT_SECRET"),
            oauth_redirect_uri: env_required("OAUTH_REDIRECT_URI"),
            jwt_secret: env_required("JWT_SECRET"),
            api_port,
            frontend_origin: env_or("FRONTEND_ORIGIN", "http://localhost:8080"),
            dev_mode: env_or("DEV_MODE", "false") == "true",
        }
    }

    /// Table name with prefix applied.
    pub fn table_name(&self) -> String {
        format!("{}ogrenotes", self.dynamodb_table_prefix)
    }
}

fn env_required(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| panic!("{key} environment variable is required"))
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secrets() {
        // Can't call from_env without setting vars, but we can construct directly
        let config = AppConfig {
            aws_region: "us-east-1".into(),
            dynamodb_table_prefix: "test-".into(),
            s3_bucket: "test-bucket".into(),
            redis_url: "redis://secret@host:6379".into(),
            oauth_client_id: "client-id".into(),
            oauth_client_secret: "super-secret-value".into(),
            oauth_redirect_uri: "http://localhost/callback".into(),
            jwt_secret: "my-jwt-secret-key".into(),
            api_port: 3000,
            frontend_origin: "http://localhost:8080".into(),
            dev_mode: false,
        };
        let debug_output = format!("{config:?}");
        assert!(!debug_output.contains("super-secret-value"));
        assert!(!debug_output.contains("my-jwt-secret-key"));
        assert!(!debug_output.contains("secret@host"));
        assert!(debug_output.contains("[redacted]"));
        // Non-secret fields should still be visible
        assert!(debug_output.contains("us-east-1"));
        assert!(debug_output.contains("test-bucket"));
    }
}
