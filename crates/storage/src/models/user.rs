use serde::{Deserialize, Serialize};

/// User profile stored in DynamoDB.
/// PK: USER#<user_id>, SK: PROFILE
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct User {
    pub user_id: String,
    pub name: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,

    // System folder IDs (created on first login)
    pub home_folder_id: String,
    pub private_folder_id: String,
    pub trash_folder_id: String,
    /// Archive folder (Phase 2): documents removed from active view but not deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_folder_id: Option<String>,
    /// Pinned folder (Phase 2): starred/favorited documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_folder_id: Option<String>,

    pub created_at: i64,
    pub updated_at: i64,
}

impl User {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk() -> &'static str {
        "PROFILE"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_common::id::new_id;
    use ogrenotes_common::time::now_usec;

    fn sample_user() -> User {
        let now = now_usec();
        User {
            user_id: new_id(),
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            avatar_url: Some("https://example.com/avatar.png".to_string()),
            home_folder_id: new_id(),
            private_folder_id: new_id(),
            trash_folder_id: new_id(),
            archive_folder_id: Some(new_id()),
            pinned_folder_id: Some(new_id()),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn user_pk_format() {
        let user = sample_user();
        assert!(user.pk().starts_with("USER#"));
        assert_eq!(user.pk(), format!("USER#{}", user.user_id));
    }

    #[test]
    fn user_sk_format() {
        assert_eq!(User::sk(), "PROFILE");
    }

    #[test]
    fn user_json_roundtrip() {
        let user = sample_user();
        let json = serde_json::to_string(&user).unwrap();
        let back: User = serde_json::from_str(&json).unwrap();
        assert_eq!(user, back);
    }

    #[test]
    fn user_avatar_url_optional() {
        let mut user = sample_user();
        user.avatar_url = None;
        let json = serde_json::to_string(&user).unwrap();
        assert!(!json.contains("avatar_url"));
        let back: User = serde_json::from_str(&json).unwrap();
        assert_eq!(back.avatar_url, None);
    }
}
