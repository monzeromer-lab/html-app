//! `sql` — bundled SQLite (docs/api-reference.md, Tier 1).
//!
//! Only the databases named in the manifest can be opened. SQLite is synchronous and a `Connection`
//! is not `Sync`, so connections live on a dedicated blocking thread per database and the async
//! side talks to them over a channel — which also serialises access, so two concurrent bridge calls
//! cannot interleave inside one transaction.

use std::collections::HashMap;
use std::path::PathBuf;

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

pub struct SqlModule {
    ctx: Ctx,
    #[cfg(feature = "tier1")]
    open: Mutex<HashMap<PathBuf, Connection>>,
}

#[cfg(feature = "tier1")]
struct Connection {
    requests: tokio::sync::mpsc::UnboundedSender<Job>,
}

#[cfg(feature = "tier1")]
type Job = Box<dyn FnOnce(&mut rusqlite::Connection) + Send>;

#[derive(Deserialize)]
struct DatabaseParams {
    database: String,
}

#[derive(Deserialize)]
struct QueryParams {
    database: String,
    sql: String,
    #[serde(default)]
    params: Vec<Value>,
}

#[derive(Deserialize)]
struct TransactionParams {
    database: String,
    statements: Vec<Statement>,
}

#[derive(Deserialize)]
struct Statement {
    sql: String,
    #[serde(default)]
    params: Vec<Value>,
}

impl SqlModule {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            #[cfg(feature = "tier1")]
            open: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(feature = "tier1")]
impl SqlModule {
    /// Get or start the worker thread for one database, after checking the manifest.
    fn connect(&self, database: &str) -> Result<tokio::sync::mpsc::UnboundedSender<Job>, RpcError> {
        let path = self.ctx.check_database(database)?;

        if let Some(existing) = self.open.lock().get(&path) {
            return Ok(existing.requests.clone());
        }

        let connection = rusqlite::Connection::open(&path).map_err(|e| {
            RpcError::new(
                htmlapp_bridge::ErrorCode::OperationFailed,
                format!("could not open {}: {e}", path.display()),
            )
        })?;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Job>();
        std::thread::spawn(move || {
            let mut connection = connection;
            while let Some(job) = rx.blocking_recv() {
                job(&mut connection);
            }
        });

        self.open.lock().insert(
            path,
            Connection {
                requests: tx.clone(),
            },
        );
        Ok(tx)
    }

    /// Run a closure against the connection thread and await its result.
    async fn with_connection<T, F>(&self, database: &str, work: F) -> Result<T, RpcError>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection) -> Result<T, RpcError> + Send + 'static,
    {
        let requests = self.connect(database)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        requests
            .send(Box::new(move |connection| {
                let _ = tx.send(work(connection));
            }))
            .map_err(|_| RpcError::internal("database connection closed"))?;
        rx.await
            .map_err(|_| RpcError::internal("database worker stopped"))?
    }
}

/// Convert a JSON parameter into something SQLite can bind.
#[cfg(feature = "tier1")]
fn to_sql(value: &Value) -> Result<rusqlite::types::Value, RpcError> {
    use rusqlite::types::Value as Sql;
    Ok(match value {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(*b as i64),
        Value::Number(n) if n.is_i64() => Sql::Integer(n.as_i64().unwrap()),
        Value::Number(n) => Sql::Real(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => Sql::Text(s.clone()),
        other => {
            return Err(RpcError::invalid_params(format!(
                "cannot bind {other} as a SQL parameter"
            )));
        }
    })
}

#[cfg(feature = "tier1")]
fn from_sql(value: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => {
            use base64::Engine as _;
            json!(base64::engine::general_purpose::STANDARD.encode(b))
        }
    }
}

#[cfg(feature = "tier1")]
fn run_query(
    connection: &rusqlite::Connection,
    sql: &str,
    params: &[Value],
) -> Result<Vec<Value>, RpcError> {
    let bound: Vec<rusqlite::types::Value> =
        params.iter().map(to_sql).collect::<Result<_, _>>()?;

    let mut statement = connection
        .prepare(sql)
        .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
    let columns: Vec<String> = statement.column_names().iter().map(|c| c.to_string()).collect();

    let mut rows = statement
        .query(rusqlite::params_from_iter(bound))
        .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?
    {
        let mut object = serde_json::Map::new();
        for (i, name) in columns.iter().enumerate() {
            let value = row.get_ref(i).map(from_sql).unwrap_or(Value::Null);
            object.insert(name.clone(), value);
        }
        out.push(Value::Object(object));
    }
    Ok(out)
}

impl ApiHandler for SqlModule {
    fn name(&self) -> &'static str {
        "sql"
    }

    #[cfg(feature = "tier1")]
    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "open" => {
                    let params: DatabaseParams = decode("sql.open", params)?;
                    self.connect(&params.database)?;
                    Ok(Value::Null)
                }
                "close" => {
                    let params: DatabaseParams = decode("sql.close", params)?;
                    let path = self.ctx.check_database(&params.database)?;
                    self.open.lock().remove(&path);
                    Ok(Value::Null)
                }
                "query" => {
                    let params: QueryParams = decode("sql.query", params)?;
                    let (sql, bound) = (params.sql, params.params);
                    let rows = self
                        .with_connection(&params.database, move |c| run_query(c, &sql, &bound))
                        .await?;
                    Ok(json!(rows))
                }
                "execute" => {
                    let params: QueryParams = decode("sql.execute", params)?;
                    let (sql, bound) = (params.sql, params.params);
                    self.with_connection(&params.database, move |connection| {
                        let values: Vec<rusqlite::types::Value> =
                            bound.iter().map(to_sql).collect::<Result<_, _>>()?;
                        let affected = connection
                            .execute(&sql, rusqlite::params_from_iter(values))
                            .map_err(|e| RpcError::new(
                                htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                        Ok(json!({
                            "rowsAffected": affected,
                            "lastInsertId": connection.last_insert_rowid(),
                        }))
                    })
                    .await
                }
                "transaction" => {
                    let params: TransactionParams = decode("sql.transaction", params)?;
                    let statements = params.statements;
                    self.with_connection(&params.database, move |connection| {
                        let transaction = connection.transaction().map_err(|e| {
                            RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string())
                        })?;
                        for statement in &statements {
                            let values: Vec<rusqlite::types::Value> =
                                statement.params.iter().map(to_sql).collect::<Result<_, _>>()?;
                            transaction
                                .execute(&statement.sql, rusqlite::params_from_iter(values))
                                .map_err(|e| RpcError::new(
                                    htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                        }
                        // Any error above returns early and drops the transaction, which rolls it
                        // back — so a failed batch never lands half-applied.
                        transaction.commit().map_err(|e| {
                            RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string())
                        })?;
                        Ok(Value::Null)
                    })
                    .await
                }
                other => Err(RpcError::not_found(&format!("sql.{other}"))),
            }
        })
    }

    #[cfg(not(feature = "tier1"))]
    fn invoke<'a>(&'a self, method: &'a str, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        let method = method.to_string();
        Box::pin(async move {
            let _ = method;
            Err(RpcError::unsupported("this build has no SQL support"))
        })
    }

    #[cfg(feature = "tier1")]
    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            if method != "stream" {
                return Err(RpcError::not_found(&format!("sql.{method}")));
            }
            let params: QueryParams = decode("sql.stream", params)?;
            let (sql, bound) = (params.sql, params.params);
            // Rows are collected on the worker thread and then yielded one at a time. Holding a
            // `rusqlite::Rows` across an await point is not possible — it borrows the statement —
            // so this trades peak memory for a streaming *interface*, which is what the page's
            // `for await` needs.
            let rows = self
                .with_connection(&params.database, move |c| run_query(c, &sql, &bound))
                .await?;
            Ok(Box::pin(futures::stream::iter(rows.into_iter().map(Ok))) as ValueStream)
        })
    }
}
