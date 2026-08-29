//! A small JSON-RPC 2.0 client for a language server.
//!
//! The read loop runs on the background executor and forwards raw messages
//! over a channel to a foreground dispatcher, which resolves pending
//! requests and invokes the registered handlers with `&mut App` access.
//! Everything the UI touches therefore happens on the foreground thread.

use anyhow::{Context as _, Result, anyhow};
use gpui::{App, AppContext as _, Task};
use serde::Serialize;
use serde_json::{Value, json};
use smol::io::{AsyncRead, AsyncWrite};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    process::Stdio,
    rc::Rc,
};

use crate::transport::{MessageReader, MessageWriter};

pub type RequestHandler = Rc<dyn Fn(Value, &mut App) -> Result<Value>>;
pub type NotificationHandler = Rc<dyn Fn(Value, &mut App)>;

struct ClientState {
    next_id: Cell<i64>,
    outgoing_tx: async_channel::Sender<Vec<u8>>,
    pending: RefCell<HashMap<i64, async_channel::Sender<Result<Value>>>>,
    request_handlers: RefCell<HashMap<&'static str, RequestHandler>>,
    notification_handlers: RefCell<HashMap<&'static str, NotificationHandler>>,
    capabilities: RefCell<Option<lsp_types::ServerCapabilities>>,
    _io_tasks: RefCell<Vec<Task<()>>>,
}

/// A cloneable handle to a running language server connection.
#[derive(Clone)]
pub struct LspClient {
    state: Rc<ClientState>,
}

impl LspClient {
    /// Build a client over a raw duplex byte stream, spawning the reader,
    /// writer, and dispatcher tasks.
    pub fn new<R, W>(reader: R, writer: W, cx: &mut App) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing_tx, outgoing_rx) = async_channel::unbounded::<Vec<u8>>();
        let (incoming_tx, incoming_rx) = async_channel::unbounded::<Vec<u8>>();

        let writer_task = cx.background_spawn(async move {
            let mut writer = MessageWriter::new(writer);
            while let Ok(payload) = outgoing_rx.recv().await {
                if writer.write(&payload).await.is_err() {
                    break;
                }
            }
        });

        let reader_task = cx.background_spawn(async move {
            let mut reader = MessageReader::new(reader);
            while let Ok(Some(payload)) = reader.read().await {
                if incoming_tx.send(payload).await.is_err() {
                    break;
                }
            }
        });

        let state = Rc::new(ClientState {
            next_id: Cell::new(0),
            outgoing_tx,
            pending: RefCell::new(HashMap::new()),
            request_handlers: RefCell::new(HashMap::new()),
            notification_handlers: RefCell::new(HashMap::new()),
            capabilities: RefCell::new(None),
            _io_tasks: RefCell::new(vec![]),
        });

        let client = Self {
            state: state.clone(),
        };
        let dispatcher = client.clone();
        let dispatcher_task = cx.spawn(async move |cx| {
            while let Ok(payload) = incoming_rx.recv().await {
                let Ok(message) = serde_json::from_slice::<Value>(&payload) else {
                    continue;
                };
                cx.update(|cx| dispatcher.dispatch(message, cx));
            }
        });
        state
            ._io_tasks
            .borrow_mut()
            .extend([writer_task, reader_task, dispatcher_task]);

        client
    }

    /// Spawn a language server process and connect over its stdio. The
    /// returned child must be kept alive alongside the client.
    pub fn connect_to_command(
        mut command: smol::process::Command,
        cx: &mut App,
    ) -> Result<(Self, smol::process::Child)> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().context("failed to spawn language server")?;
        let stdout = child.stdout.take().context("missing child stdout")?;
        let stdin = child.stdin.take().context("missing child stdin")?;
        Ok((Self::new(stdout, stdin, cx), child))
    }

    /// The capabilities the server reported in the initialize handshake.
    pub fn capabilities(&self) -> Option<lsp_types::ServerCapabilities> {
        self.state.capabilities.borrow().clone()
    }

    /// Send a request; the returned future resolves with the typed result.
    ///
    /// The message is written when the future is first polled, not when it
    /// is created. Providers fire requests from inside an edit, before the
    /// `InputEvent::Change` subscription has sent the `didChange` for that
    /// edit; deferring the write to the next executor turn — after the
    /// update's effects have flushed — keeps the document sync ahead of
    /// every request that positions into it.
    pub fn request<R: lsp_types::request::Request>(
        &self,
        params: R::Params,
    ) -> impl Future<Output = Result<R::Result>> + use<R> {
        let id = self.state.next_id.get();
        self.state.next_id.set(id + 1);

        let (tx, rx) = async_channel::bounded(1);
        self.state.pending.borrow_mut().insert(id, tx);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": R::METHOD,
            "params": params,
        });

        let this = self.clone();
        async move {
            this.send(message)?;
            let value = rx
                .recv()
                .await
                .map_err(|_| anyhow!("language server connection closed"))??;
            Ok(serde_json::from_value(value)?)
        }
    }

    /// Send a notification.
    pub fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        let _ = self.send(json!({
            "jsonrpc": "2.0",
            "method": N::METHOD,
            "params": params,
        }));
    }

    /// Register a handler for a server-to-client request.
    pub fn on_request<R, F>(&self, handler: F)
    where
        R: lsp_types::request::Request,
        F: Fn(R::Params, &mut App) -> Result<R::Result> + 'static,
    {
        self.state.request_handlers.borrow_mut().insert(
            R::METHOD,
            Rc::new(move |params, cx| {
                let params = serde_json::from_value(params)?;
                Ok(serde_json::to_value(handler(params, cx)?)?)
            }),
        );
    }

    /// Register a handler for a server-to-client notification.
    pub fn on_notification<N, F>(&self, handler: F)
    where
        N: lsp_types::notification::Notification,
        F: Fn(N::Params, &mut App) + 'static,
    {
        self.state.notification_handlers.borrow_mut().insert(
            N::METHOD,
            Rc::new(move |params, cx| {
                if let Ok(params) = serde_json::from_value(params) {
                    handler(params, cx);
                }
            }),
        );
    }

    /// Run the `initialize` handshake: send the request, store the server's
    /// capabilities, and confirm with the `initialized` notification.
    pub fn initialize(
        &self,
        params: lsp_types::InitializeParams,
    ) -> impl Future<Output = Result<lsp_types::InitializeResult>> + use<> {
        let this = self.clone();
        let request = self.request::<lsp_types::request::Initialize>(params);
        async move {
            let result = request.await?;
            *this.state.capabilities.borrow_mut() = Some(result.capabilities.clone());
            this.notify::<lsp_types::notification::Initialized>(lsp_types::InitializedParams {});
            Ok(result)
        }
    }

    /// Run the shutdown sequence: the `shutdown` request followed by the
    /// `exit` notification.
    pub fn shutdown(&self) -> impl Future<Output = Result<()>> + use<> {
        let this = self.clone();
        let request = self.request::<lsp_types::request::Shutdown>(());
        async move {
            request.await?;
            this.notify::<lsp_types::notification::Exit>(());
            Ok(())
        }
    }

    fn send(&self, message: impl Serialize) -> Result<()> {
        let payload = serde_json::to_vec(&message)?;
        self.state
            .outgoing_tx
            .try_send(payload)
            .map_err(|_| anyhow!("language server connection closed"))
    }

    fn dispatch(&self, message: Value, cx: &mut App) {
        let id = message.get("id").and_then(Value::as_i64);
        let method = message.get("method").and_then(Value::as_str);

        match (id, method) {
            // A response to one of our requests.
            (Some(id), None) => {
                let Some(tx) = self.state.pending.borrow_mut().remove(&id) else {
                    return;
                };
                let result = if let Some(error) = message.get("error") {
                    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                    let text = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error");
                    Err(anyhow!("language server error {code}: {text}"))
                } else {
                    Ok(message.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = tx.try_send(result);
            }
            // A server-to-client request: invoke the handler and respond.
            (Some(id), Some(method)) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let handler = self.state.request_handlers.borrow().get(method).cloned();
                let response = match handler {
                    Some(handler) => match handler(params, cx) {
                        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                        Err(error) => json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32603, "message": error.to_string()},
                        }),
                    },
                    None => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": format!("method not found: {method}")},
                    }),
                };
                let _ = self.send(response);
            }
            // A server notification.
            (None, Some(method)) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let handler = self
                    .state
                    .notification_handlers
                    .borrow()
                    .get(method)
                    .cloned();
                if let Some(handler) = handler {
                    handler(params, cx);
                }
            }
            (None, None) => {}
        }
    }
}
