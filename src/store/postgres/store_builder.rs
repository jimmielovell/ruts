use crate::store::Error;
use crate::store::postgres::PostgresStore;
use sqlx::PgPool;
use std::time::Duration;

fn validate_identifier(name: &str) -> Result<(), Error> {
    if name.is_empty() || name.len() > 63 {
        return Err(Error::Backend(format!(
            "invalid identifier {name:?}: must be 1-63 bytes"
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(Error::Backend(format!(
            "invalid identifier {name:?}: must start with a letter or underscore"
        )));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::Backend(format!(
            "invalid identifier {name:?}: only ASCII alphanumerics and underscore allowed"
        )));
    }
    Ok(())
}

/// A builder for creating a `PostgresStore`.
///
/// This allows for customizing the table and schema names for session storage.
pub struct PostgresStoreBuilder {
    pool: PgPool,
    table_name: String,
    create_table: bool,
    schema_name: Option<String>,
    cleanup_interval: Option<Duration>,
}

impl PostgresStoreBuilder {
    /// Creates a new builder with a database pool and default settings.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            table_name: "t_sessions".to_string(),
            create_table: false,
            schema_name: None,
            cleanup_interval: None,
        }
    }

    /// Create the session tables (and schema, if configured) on `build()`.
    ///
    /// Defaults to `false`. Enable for development or when no external
    /// migration system manages the schema.
    pub fn create_table(mut self, create: bool) -> Self {
        self.create_table = create;
        self
    }

    /// Sets a custom table name for the session store. Defaults to "t_sessions".
    pub fn table_name(mut self, table_name: impl Into<String>) -> Result<Self, Error> {
        let name = table_name.into();
        validate_identifier(&name)?;
        self.table_name = name;
        Ok(self)
    }

    /// Sets a custom schema name for the session store.
    pub fn schema_name(mut self, schema_name: impl Into<String>) -> Result<Self, Error> {
        let name = schema_name.into();
        validate_identifier(&name)?;
        self.schema_name = Some(name);
        Ok(self)
    }

    /// Sets the interval for the background task that cleans up expired sessions.
    ///
    /// If this is not set, the cleanup task defaults to running every 5 minutes.
    pub fn cleanup_interval(mut self, interval: Duration) -> Self {
        self.cleanup_interval = Some(interval);
        self
    }

    /// Builds the `PostgresStore`, creating the schema and table if they don't exist.
    pub async fn build(self) -> Result<PostgresStore, sqlx::Error> {
        let table_name = if let Some(schema) = &self.schema_name {
            format!("\"{}\".\"{}\"", schema, self.table_name)
        } else {
            format!("\"{}\"", self.table_name)
        };

        if self.create_table {
            if let Some(schema) = &self.schema_name {
                sqlx::query(&format!("create schema if not exists \"{schema}\""))
                    .execute(&self.pool)
                    .await?;
            }

            sqlx::raw_sql(&format!(
                r#"
                create table if not exists {table_name} (
                    session_id text not null,
                    field text not null,
                    value bytea not null,
                    hot_cache_ttl bigint,
                    expires_at timestamptz not null,
                    primary key (session_id, field)
                );

                create index if not exists idx_sessions_session_id on {table_name}(session_id);
                create index if not exists idx_sessions_expires_at on {table_name}(expires_at);
                "#
            ))
            .execute(&self.pool)
            .await?;
        }

        let pool = self.pool.clone();
        let t_name = table_name.clone();
        let interval = self.cleanup_interval.unwrap_or(Duration::from_secs(60 * 5));

        let cleanup_task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let query = format!("delete from {t_name} where expires_at < now()");

            loop {
                ticker.tick().await;
                let _ = sqlx::query(&query).execute(&pool).await;
            }
        });

        Ok(PostgresStore {
            pool: self.pool,
            table_name,
            cleanup_task: Some(cleanup_task),
        })
    }
}
