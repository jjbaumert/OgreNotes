use serde::{Deserialize, Serialize};

use super::client::{api_delete, api_get, api_patch, api_post, ApiClientError};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderResponse {
    pub id: String,
    pub title: String,
    pub color: u8,
    pub parent_id: Option<String>,
    pub folder_type: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub children: Vec<ChildResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildResponse {
    pub child_id: String,
    pub child_type: String,
    pub title: String,
    pub added_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateFolderRequest {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
}

pub async fn create_folder(
    title: &str,
    parent_id: Option<&str>,
) -> Result<FolderResponse, ApiClientError> {
    let body = CreateFolderRequest {
        title: title.to_string(),
        parent_id: parent_id.map(|s| s.to_string()),
    };
    api_post("/folders", &body).await
}

pub async fn get_folder(id: &str) -> Result<FolderResponse, ApiClientError> {
    api_get(&format!("/folders/{id}")).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateFolderRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<u8>,
}

pub async fn update_folder(
    id: &str,
    title: Option<&str>,
    color: Option<u8>,
) -> Result<(), ApiClientError> {
    let body = UpdateFolderRequest {
        title: title.map(|s| s.to_string()),
        color,
    };
    api_patch(&format!("/folders/{id}"), &body).await
}

pub async fn delete_folder(id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/folders/{id}")).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddChildRequest {
    child_id: String,
    child_type: String,
}

pub async fn add_child(
    folder_id: &str,
    child_id: &str,
    child_type: &str,
) -> Result<(), ApiClientError> {
    let body = AddChildRequest {
        child_id: child_id.to_string(),
        child_type: child_type.to_string(),
    };
    api_post::<serde_json::Value, _>(&format!("/folders/{folder_id}/children"), &body)
        .await
        .map(|_| ())
}

pub async fn remove_child(folder_id: &str, child_id: &str) -> Result<(), ApiClientError> {
    api_delete(&format!("/folders/{folder_id}/children/{child_id}")).await
}
