//! `store` — a persistent key-value store scoped to the app id (docs/api-reference.md, Tier 1).
//!
//! The 80% case that does not need SQL. Scoped by app id so two documents never share a namespace,
//! and written atomically so a crash mid-write cannot leave an app's settings truncated.

use std::collections::BTreeMap;

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

pub struct StoreModule {
    ctx: Ctx,
    cache: Mutex<Option<BTreeMap<String, Value>>>,
}

#[derive(Deserialize)]
struct KeyParams {
    key: String,
}

#[derive(Deserialize)]
struct SetParams {
    key: String,
    value: Value,
}

impl StoreModule {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            cache: Mutex::new(None),
        }
    }

    fn path(&self) -> Result<std::path::PathBuf, RpcError> {
        Ok(self.ctx.data_dir()?.join("store.json"))
    }

    fn load(&self) -> Result<BTreeMap<String, Value>, RpcError> {
        if let Some(cached) = self.cache.lock().as_ref() {
            return Ok(cached.clone());
        }
        let path = self.path()?;
        let map = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(RpcError::internal(format!("could not read store: {e}"))),
        };
        *self.cache.lock() = Some(map.clone());
        Ok(map)
    }

    fn save(&self, map: BTreeMap<String, Value>) -> Result<(), RpcError> {
        let path = self.path()?;
        let json =
            serde_json::to_string_pretty(&map).map_err(|e| RpcError::internal(e.to_string()))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, json).map_err(|e| RpcError::internal(e.to_string()))?;
        std::fs::rename(&temp, &path).map_err(|e| RpcError::internal(e.to_string()))?;
        *self.cache.lock() = Some(map);
        Ok(())
    }
}

impl ApiHandler for StoreModule {
    fn name(&self) -> &'static str {
        "store"
    }

    fn invoke<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "get" => {
                    let params: KeyParams = decode("store.get", params)?;
                    Ok(self
                        .load()?
                        .get(&params.key)
                        .cloned()
                        .unwrap_or(Value::Null))
                }
                "set" => {
                    let params: SetParams = decode("store.set", params)?;
                    let mut map = self.load()?;
                    map.insert(params.key, params.value);
                    self.save(map)?;
                    Ok(Value::Null)
                }
                "delete" => {
                    let params: KeyParams = decode("store.delete", params)?;
                    let mut map = self.load()?;
                    map.remove(&params.key);
                    self.save(map)?;
                    Ok(Value::Null)
                }
                "keys" => Ok(json!(self.load()?.keys().collect::<Vec<_>>())),
                "clear" => {
                    self.save(BTreeMap::new())?;
                    Ok(Value::Null)
                }
                other => Err(RpcError::not_found(&format!("store.{other}"))),
            }
        })
    }
}
