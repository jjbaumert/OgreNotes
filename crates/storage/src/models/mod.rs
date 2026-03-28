pub mod document;
pub mod folder;
pub mod session;
pub mod user;

/// Access levels for folder/document membership.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum AccessLevel {
    Own,
    Edit,
    Comment,
    View,
}

/// Document types.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocType {
    Document,
    Spreadsheet,
    Chat,
}

impl DocType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocType::Document => "document",
            DocType::Spreadsheet => "spreadsheet",
            DocType::Chat => "chat",
        }
    }
}

/// Folder types.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FolderType {
    System,
    User,
}

impl FolderType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FolderType::System => "system",
            FolderType::User => "user",
        }
    }
}

/// Child types for folder membership.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChildType {
    Doc,
    Folder,
}

impl ChildType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChildType::Doc => "doc",
            ChildType::Folder => "folder",
        }
    }
}
