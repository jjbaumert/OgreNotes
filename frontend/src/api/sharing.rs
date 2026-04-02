use serde::Deserialize;
use super::client::{api_get, api_post_empty, api_delete, ApiClientError};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberItem {
    pub user_id: String,
    pub name: String,
    #[serde(default)]
    pub email: String,
    pub access_level: String,
    pub added_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MembersListResponse {
    pub members: Vec<MemberItem>,
}

pub async fn list_members(folder_id: &str) -> Result<MembersListResponse, ApiClientError> {
    api_get(&format!("/folders/{folder_id}/members")).await
}

pub async fn add_member(
    folder_id: &str,
    user_id: &str,
    access_level: &str,
) -> Result<(), ApiClientError> {
    api_post_empty(
        &format!("/folders/{folder_id}/members"),
        &serde_json::json!({
            "userId": user_id,
            "accessLevel": access_level
        }),
    )
    .await
}

pub async fn remove_member(folder_id: &str, user_id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/folders/{folder_id}/members/{user_id}")).await
}
