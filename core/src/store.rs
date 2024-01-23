use async_trait::async_trait;

pub enum StoreError {
    NotFound,
    Other(String),
}

pub struct Record {
    pub id: String,
    pub data: String,
}

#[async_trait]
pub trait SessionStore: Clone + Send + Sync + 'static {
    async fn load(&self, id: &str) -> Option<Record>;
    async fn save(&self, record: Record) -> Result<(), StoreError>;
    async fn delete(&self, id: &str) -> Result<(), StoreError>;
}
