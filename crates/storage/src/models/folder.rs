use serde::{Deserialize, Serialize};

use super::{AccessLevel, ChildType, FolderType};

/// Folder metadata stored in DynamoDB.
/// PK: FOLDER#<folder_id>, SK: METADATA
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Folder {
    pub folder_id: String,
    pub title: String,
    pub color: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub owner_id: String,
    pub folder_type: FolderType,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Folder {
    pub fn pk(&self) -> String {
        format!("FOLDER#{}", self.folder_id)
    }

    pub fn sk() -> &'static str {
        "METADATA"
    }

    /// Clamp color to the valid range 0-11.
    pub fn clamp_color(color: u8) -> u8 {
        color.min(11)
    }
}

/// A child entry in a folder (document or subfolder).
/// PK: FOLDER#<folder_id>, SK: CHILD#<child_id>
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderChild {
    pub folder_id: String,
    pub child_id: String,
    pub child_type: ChildType,
    pub added_at: i64,
}

impl FolderChild {
    pub fn pk(&self) -> String {
        format!("FOLDER#{}", self.folder_id)
    }

    pub fn sk(&self) -> String {
        format!("CHILD#{}", self.child_id)
    }
}

/// A membership entry in a folder (user with access level).
/// PK: FOLDER#<folder_id>, SK: MEMBER#<user_id>
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderMember {
    pub folder_id: String,
    pub user_id: String,
    pub access_level: AccessLevel,
    pub added_at: i64,
}

impl FolderMember {
    pub fn pk(&self) -> String {
        format!("FOLDER#{}", self.folder_id)
    }

    pub fn sk(&self) -> String {
        format!("MEMBER#{}", self.user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_common::id::new_id;
    use ogrenotes_common::time::now_usec;

    fn sample_folder() -> Folder {
        let now = now_usec();
        Folder {
            folder_id: new_id(),
            title: "My Folder".to_string(),
            color: 4,
            parent_id: None,
            owner_id: new_id(),
            folder_type: FolderType::User,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn folder_pk_format() {
        let folder = sample_folder();
        assert_eq!(folder.pk(), format!("FOLDER#{}", folder.folder_id));
    }

    #[test]
    fn folder_json_roundtrip() {
        let folder = sample_folder();
        let json = serde_json::to_string(&folder).unwrap();
        let back: Folder = serde_json::from_str(&json).unwrap();
        assert_eq!(folder, back);
    }

    #[test]
    fn folder_color_clamp() {
        assert_eq!(Folder::clamp_color(0), 0);
        assert_eq!(Folder::clamp_color(11), 11);
        assert_eq!(Folder::clamp_color(12), 11);
        assert_eq!(Folder::clamp_color(255), 11);
    }

    #[test]
    fn folder_child_pk_sk_format() {
        let child = FolderChild {
            folder_id: "folder1".to_string(),
            child_id: "doc1".to_string(),
            child_type: ChildType::Doc,
            added_at: now_usec(),
        };
        assert_eq!(child.pk(), "FOLDER#folder1");
        assert_eq!(child.sk(), "CHILD#doc1");
    }

    #[test]
    fn folder_member_pk_sk_format() {
        let member = FolderMember {
            folder_id: "folder1".to_string(),
            user_id: "user1".to_string(),
            access_level: AccessLevel::Own,
            added_at: now_usec(),
        };
        assert_eq!(member.pk(), "FOLDER#folder1");
        assert_eq!(member.sk(), "MEMBER#user1");
    }

    #[test]
    fn folder_type_serialization() {
        let folder = sample_folder();
        let json = serde_json::to_string(&folder).unwrap();
        assert!(json.contains("\"folder_type\":\"user\""));

        let mut system_folder = folder;
        system_folder.folder_type = FolderType::System;
        let json = serde_json::to_string(&system_folder).unwrap();
        assert!(json.contains("\"folder_type\":\"system\""));
    }
}
