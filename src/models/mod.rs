use serde::{Serialize, Deserialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Business {
    pub id: Uuid,
    pub name: String,
    pub api_key_hash: String,
    pub webhook_secret: String,
    pub webhook_url: Option<String>,
}