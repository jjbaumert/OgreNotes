use serde::{Deserialize, Serialize};

/// Session record stored in DynamoDB.
/// PK: USER#<user_id>, SK: SESSION#<session_id>
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub user_id: String,
    pub session_id: String,
    pub refresh_token_hash: String,
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_info: Option<String>,
    pub created_at: i64,
}

impl Session {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk(&self) -> String {
        format!("SESSION#{}", self.session_id)
    }

    /// Check if the session has expired.
    pub fn is_expired(&self) -> bool {
        ogrenotes_common::time::now_usec() > self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_common::id::new_id;
    use ogrenotes_common::time::now_usec;

    fn sample_session() -> Session {
        let now = now_usec();
        Session {
            user_id: new_id(),
            session_id: new_id(),
            refresh_token_hash: "sha256hash_placeholder".to_string(),
            expires_at: now + 30 * 24 * 3600 * 1_000_000, // 30 days in usec
            device_info: Some("Chrome/Linux".to_string()),
            created_at: now,
        }
    }

    #[test]
    fn session_pk_format() {
        let session = sample_session();
        assert_eq!(session.pk(), format!("USER#{}", session.user_id));
    }

    #[test]
    fn session_sk_format() {
        let session = sample_session();
        assert_eq!(session.sk(), format!("SESSION#{}", session.session_id));
    }

    #[test]
    fn session_json_roundtrip() {
        let session = sample_session();
        let json = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(session, back);
    }

    #[test]
    fn session_not_expired() {
        let session = sample_session();
        assert!(!session.is_expired());
    }

    #[test]
    fn session_expired() {
        let mut session = sample_session();
        session.expires_at = now_usec() - 1_000_000; // 1 second ago
        assert!(session.is_expired());
    }
}
