//! `secrets` — the Secret Service (PRD §9.3 Tier 2).
//!
//! "Credentials never touch the page's storage." A secret goes to the user's keyring — GNOME
//! Keyring, KWallet, or anything else implementing `org.freedesktop.secrets` — and comes back only
//! when asked for by name.
//!
//! Spoken over D-Bus directly rather than through `libsecret`: the C library is not installed on
//! every desktop, and a credential API that silently does not exist is worse than one that works
//! anywhere the keyring daemon is running.

use std::collections::HashMap;

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
const ITEM_IFACE: &str = "org.freedesktop.Secret.Item";
/// The user's default collection, whatever they have called it.
const DEFAULT_COLLECTION: &str = "/org/freedesktop/secrets/aliases/default";

pub struct SecretsModule {
    #[allow(dead_code)]
    ctx: Ctx,
    /// Secrets are namespaced by app id, so two documents cannot read each other's credentials
    /// even though they share one keyring.
    app_id: String,
}

#[derive(Deserialize)]
struct KeyParams {
    key: String,
}

#[derive(Deserialize)]
struct SetParams {
    key: String,
    value: String,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
struct SearchParams {
    attributes: HashMap<String, String>,
}

impl SecretsModule {
    pub fn new(ctx: Ctx, app_id: impl Into<String>) -> Self {
        Self {
            ctx,
            app_id: app_id.into(),
        }
    }

    /// The attribute set that identifies one of this document's secrets.
    fn attributes(&self, key: &str) -> HashMap<String, String> {
        HashMap::from([
            ("xdg:schema".to_string(), "app.htmlapp.Secret".to_string()),
            ("application".to_string(), self.app_id.clone()),
            ("key".to_string(), key.to_string()),
        ])
    }
}

#[cfg(feature = "tier2")]
mod dbus_impl {
    use super::*;
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value as ZValue};

    /// A Secret Service `Secret` struct: (session, parameters, value, content_type).
    type SecretStruct<'a> = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

    impl SecretsModule {
        pub(super) async fn connect(&self) -> Result<zbus::Connection, RpcError> {
            zbus::Connection::session()
                .await
                .map_err(|e| RpcError::internal(format!("no session bus: {e}")))
        }

        /// Open a plaintext session. The transport to the daemon is a local Unix socket, so the
        /// algorithm negotiation that DH would buy is not protecting anything a local attacker
        /// could not already reach.
        async fn open_session(
            &self,
            connection: &zbus::Connection,
        ) -> Result<OwnedObjectPath, RpcError> {
            let reply = connection
                .call_method(
                    Some(SERVICE),
                    SERVICE_PATH,
                    Some(SERVICE_IFACE),
                    "OpenSession",
                    &("plain", ZValue::Str("".into())),
                )
                .await
                .map_err(operation_failed)?;
            let (_output, session): (zbus::zvariant::OwnedValue, OwnedObjectPath) =
                reply.body().deserialize().map_err(internal)?;
            Ok(session)
        }

        /// Find the item paths matching an attribute set.
        async fn search(
            &self,
            connection: &zbus::Connection,
            attributes: &HashMap<String, String>,
        ) -> Result<Vec<OwnedObjectPath>, RpcError> {
            let reply = connection
                .call_method(
                    Some(SERVICE),
                    SERVICE_PATH,
                    Some(SERVICE_IFACE),
                    "SearchItems",
                    &(attributes,),
                )
                .await
                .map_err(operation_failed)?;
            let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
                reply.body().deserialize().map_err(internal)?;

            // Locked items are reported too: the caller may still want to know a secret exists.
            Ok(unlocked.into_iter().chain(locked).collect())
        }

        pub(super) async fn get(&self, params: Value) -> Result<Value, RpcError> {
            let params: KeyParams = decode("secrets.get", params)?;
            let connection = self.connect().await?;
            let items = self.search(&connection, &self.attributes(&params.key)).await?;

            let Some(item) = items.first() else {
                return Ok(Value::Null);
            };

            let session = self.open_session(&connection).await?;
            let reply = connection
                .call_method(
                    Some(SERVICE),
                    item.as_ref(),
                    Some(ITEM_IFACE),
                    "GetSecret",
                    &(&session,),
                )
                .await
                .map_err(operation_failed)?;

            let secret: SecretStruct = reply.body().deserialize().map_err(internal)?;
            Ok(json!(String::from_utf8_lossy(&secret.2)))
        }

        pub(super) async fn set(&self, params: Value) -> Result<Value, RpcError> {
            let params: SetParams = decode("secrets.set", params)?;
            let connection = self.connect().await?;
            let session = self.open_session(&connection).await?;

            let attributes = self.attributes(&params.key);
            let label = params
                .label
                .unwrap_or_else(|| format!("{} — {}", self.app_id, params.key));

            let mut properties: HashMap<&str, ZValue<'_>> = HashMap::new();
            properties.insert("org.freedesktop.Secret.Item.Label", ZValue::from(label));
            properties.insert(
                "org.freedesktop.Secret.Item.Attributes",
                ZValue::from(attributes),
            );

            let secret: SecretStruct = (
                session,
                Vec::new(),
                params.value.into_bytes(),
                "text/plain".to_string(),
            );

            let collection = ObjectPath::try_from(DEFAULT_COLLECTION).map_err(internal)?;
            connection
                .call_method(
                    Some(SERVICE),
                    &collection,
                    Some(COLLECTION_IFACE),
                    "CreateItem",
                    // `true` replaces an existing item with the same attributes, which is what
                    // "set" should mean — otherwise every write would add a duplicate.
                    &(properties, secret, true),
                )
                .await
                .map_err(operation_failed)?;
            Ok(Value::Null)
        }

        pub(super) async fn delete(&self, params: Value) -> Result<Value, RpcError> {
            let params: KeyParams = decode("secrets.delete", params)?;
            let connection = self.connect().await?;
            for item in self.search(&connection, &self.attributes(&params.key)).await? {
                connection
                    .call_method(Some(SERVICE), item.as_ref(), Some(ITEM_IFACE), "Delete", &())
                    .await
                    .map_err(operation_failed)?;
            }
            Ok(Value::Null)
        }

        pub(super) async fn search_keys(&self, params: Value) -> Result<Value, RpcError> {
            let params: SearchParams = decode("secrets.search", params)?;
            let connection = self.connect().await?;

            // The caller's attributes are merged with this document's, never used alone: a search
            // must not be able to reach another application's items.
            let mut attributes = params.attributes;
            attributes.insert("application".into(), self.app_id.clone());
            attributes.insert("xdg:schema".into(), "app.htmlapp.Secret".into());

            let items = self.search(&connection, &attributes).await?;
            let names: Vec<String> = items
                .iter()
                .filter_map(|path| {
                    path.as_str()
                        .rsplit('/')
                        .next()
                        .map(std::string::ToString::to_string)
                })
                .collect();
            Ok(json!(names))
        }
    }

    fn operation_failed(error: zbus::Error) -> RpcError {
        RpcError::new(
            htmlapp_bridge::ErrorCode::OperationFailed,
            format!("the keyring refused: {error}"),
        )
    }

    fn internal(error: impl std::fmt::Display) -> RpcError {
        RpcError::internal(error.to_string())
    }
}

impl ApiHandler for SecretsModule {
    fn name(&self) -> &'static str {
        "secrets"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                #[cfg(feature = "tier2")]
                "get" => self.get(params).await,
                #[cfg(feature = "tier2")]
                "set" => self.set(params).await,
                #[cfg(feature = "tier2")]
                "delete" => self.delete(params).await,
                #[cfg(feature = "tier2")]
                "search" => self.search_keys(params).await,
                #[cfg(not(feature = "tier2"))]
                "get" | "set" | "delete" | "search" => {
                    let _ = &params;
                    Err(RpcError::unsupported("this build has no keyring support"))
                }
                other => Err(RpcError::not_found(&format!("secrets.{other}"))),
            }
        })
    }
}
