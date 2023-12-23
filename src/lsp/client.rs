use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::ChildStdin;

use serde_json::value::Value;
use serde_json::{self, json};

use jsonrpc_lite::{Error, Id, JsonRpc};

use super::parsing;

trait Callable: Send {
    fn call(self: Box<Self>, result: Result<Value, Value>);
}

impl<F: Send + FnOnce(Result<Value, Value>)> Callable for F {
    fn call(self: Box<F>, result: Result<Value, Value>) {
        (*self)(result)
    }
}

type Callback = Box<dyn Callable>;

/// Represents (and mediates communcation with) a Language Server.
///
/// LanguageServer should only ever be instantiated or accessed through an instance of
/// LanguageServerRef, which mediates access to a single shared LanguageServer through a Mutex.
struct LanguageServer<W: AsyncWriteExt> {
    peer: W,
    pending: HashMap<usize, Callback>,
    next_id: usize,
}

/// Generates a Language Server Protocol compliant message.
fn prepare_lsp_json(msg: &Value) -> Result<String, serde_json::error::Error> {
    let request = serde_json::to_string(&msg)?;
    Ok(format!(
        "Content-Length: {}\r\n\r\n{}",
        request.len(),
        request
    ))
}

impl<W: AsyncWriteExt + Unpin> LanguageServer<W> {
    async fn write(&mut self, msg: &str) {
        self.peer
            .write_all(msg.as_bytes())
            .await
            .expect("error writing to stdin");
        self.peer.flush().await.expect("error flushing child stdin");
    }

    async fn send_request(&mut self, method: &str, params: &Value, completion: Callback) {
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params
        });

        self.pending.insert(self.next_id, completion);
        self.next_id += 1;
        self.send_rpc(&request).await;
    }

    async fn send_notification(&mut self, method: &str, params: &Value) {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.send_rpc(&notification).await;
    }

    fn handle_response(&mut self, id: usize, result: Value) {
        let callback = self
            .pending
            .remove(&id)
            .expect(&format!("id {} missing from request table", id));
        callback.call(Ok(result));
    }

    fn handle_error(&mut self, id: usize, error: Error) {
        let callback = self
            .pending
            .remove(&id)
            .expect(&format!("id {} missing from request table", id));
        callback.call(Err(error.data.unwrap_or(serde_json::Value::Null)));
    }

    async fn send_rpc(&mut self, rpc: &Value) {
        let rpc = match prepare_lsp_json(&rpc) {
            Ok(r) => r,
            Err(err) => panic!("error encoding rpc {:?}", err),
        };
        self.write(&rpc).await;
    }
}

/// Access control and convenience wrapper around a shared LanguageServer instance.
pub struct LanguageServerRef<W: AsyncWriteExt>(Arc<Mutex<LanguageServer<W>>>);

//FIXME: this is hacky, and prevents good error propogation,
fn number_from_id(id: Option<&Value>) -> usize {
    let id = id.expect("response missing id field");
    let id = match id {
        &Value::Number(ref n) => n.as_u64().expect("failed to take id as u64"),
        &Value::String(ref s) => {
            u64::from_str_radix(s, 10).expect("failed to convert string id to u64")
        }
        other => panic!("unexpected value for id field: {:?}", other),
    };

    id as usize
}

impl<W: AsyncWriteExt + Unpin> LanguageServerRef<W> {
    fn new(peer: W) -> Self {
        LanguageServerRef(Arc::new(Mutex::new(LanguageServer {
            peer: peer,
            pending: HashMap::new(),
            next_id: 1,
        })))
    }

    fn handle_msg(&self, val: &str) {
        let parsed_value = JsonRpc::parse(val);
        if let Err(err) = parsed_value {
            println!("error parsing json: {:?}", err);
            return;
        }
        let parsed_value = parsed_value.expect("to be present");
        let id = parsed_value.get_id();
        let response = parsed_value.get_result();
        let error = parsed_value.get_error();
        match (id, response, error) {
            (Some(Id::Num(id)), Some(response), None) => {
                let mut inner = self.0.lock().unwrap();
                inner.handle_response(id.try_into().unwrap(), response.clone());
            }
            (Some(Id::Num(id)), None, Some(error)) => {
                let mut inner = self.0.lock().unwrap();
                inner.handle_error(id.try_into().unwrap(), error.clone());
            }
            (Some(Id::Num(id)), Some(response), Some(error)) => {
                panic!("We got both response and error.. what even??");
            }
            _ => {}
        }
    }

    /// Sends a JSON-RPC request message with the provided method and parameters.
    /// `completion` should be a callback which will be executed with the server's response.
    pub async fn send_request<CB>(&self, method: &str, params: &Value, completion: CB)
    where
        CB: 'static + Send + FnOnce(Result<Value, Value>),
    {
        let mut inner = self.0.lock().unwrap();
        inner
            .send_request(method, params, Box::new(completion))
            .await;
    }

    /// Sends a JSON-RPC notification message with the provided method and parameters.
    pub async fn send_notification(&self, method: &str, params: &Value) {
        let mut inner = self.0.lock().unwrap();
        inner.send_notification(method, params).await;
    }
}

impl<W: AsyncWriteExt> Clone for LanguageServerRef<W> {
    fn clone(&self) -> Self {
        LanguageServerRef(self.0.clone())
    }
}

pub async fn start_language_server(mut child: Child) -> (Child, LanguageServerRef<ChildStdin>) {
    let child_stdin = child.stdin.take().unwrap();
    let child_stdout = child.stdout.take().unwrap();
    let lang_server = LanguageServerRef::new(child_stdin);
    {
        let lang_server = lang_server.clone();
        tokio::task::spawn(async move {
            let mut reader = BufReader::new(child_stdout);
            loop {
                match parsing::read_message(&mut reader).await {
                    Ok(ref val) => lang_server.handle_msg(val),
                    Err(err) => println!("parse error: {:?}", err),
                };
            }
        });
    }
    (child, lang_server)
}
