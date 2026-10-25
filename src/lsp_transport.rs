//! Bounded stdio JSON-RPC. No language server I/O runs on the UI thread.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const QUEUE: usize = 32;
pub type Result<T> = std::result::Result<T, String>;

/// Cancellation is shared with the UI without acquiring any transport lock.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Relaxed) {
            Err("Request cancelled".into())
        } else {
            Ok(())
        }
    }
}

pub fn read_message(input: &mut impl BufRead) -> Result<Value> {
    let mut length = None;
    let mut total = 0;
    loop {
        // read_until could allocate without bound for malformed server output.
        let mut line = Vec::new();
        let n = input
            .take(8193)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        total += n;
        if n == 0 {
            return Err("Language server closed its output".into());
        }
        if total > 8192 || n > 8192 {
            return Err("Language server header too large".into());
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let line = String::from_utf8(line).map_err(|e| e.to_string())?;
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("Content-Length") {
                if length.is_some() {
                    return Err("Duplicate Content-Length".into());
                }
                length = Some(value.trim().parse::<usize>().map_err(|e| e.to_string())?);
            }
        }
    }
    let length = length.ok_or("Missing Content-Length")?;
    if length > MAX_MESSAGE {
        return Err("Language server message exceeds 16 MiB".into());
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
fn write_message(output: &mut impl Write, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_MESSAGE {
        return Err("Language server request exceeds 16 MiB".into());
    }
    write!(output, "Content-Length: {}\r\n\r\n", bytes.len()).map_err(|e| e.to_string())?;
    output
        .write_all(&bytes)
        .and_then(|_| output.flush())
        .map_err(|e| e.to_string())
}

pub struct Client {
    child: Option<Child>,
    output: SyncSender<Value>,
    input: Receiver<Result<Value>>,
    next_id: u64,
    pub capabilities: Value,
    settings: Value,
    alive: Arc<AtomicBool>,
}
impl Client {
    pub fn spawn(
        command: &mut Command,
        root_uri: &str,
        options: Value,
        settings: Value,
        cancel: &Cancellation,
    ) -> Result<Self> {
        cancel.check()?;
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Diagnostics are intentionally not piped into an unbounded buffer.
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start language server: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("Missing server stdin")?;
        let stdout = child.stdout.take().ok_or("Missing server stdout")?;
        let (output, writes) = mpsc::sync_channel::<Value>(QUEUE);
        let (received, input) = mpsc::sync_channel(QUEUE);
        let alive = Arc::new(AtomicBool::new(true));
        let writer_alive = alive.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("araseo-lsp-write".into())
            .spawn(move || {
                while let Ok(value) = writes.recv() {
                    if write_message(&mut stdin, &value).is_err() {
                        break;
                    }
                }
                writer_alive.store(false, Ordering::Relaxed);
            })
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.to_string());
        }
        let reader_alive = alive.clone();
        let replies = output.clone();
        let reader_settings = settings.clone();
        let reader_root = root_uri.to_owned();
        if let Err(error) = std::thread::Builder::new()
            .name("araseo-lsp-read".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let message = read_message(&mut reader);
                    let failed = message.is_err();
                    // Configuration and progress requests can arrive between user
                    // queries. Answer on this dispatcher so indexing never waits
                    // for the next keystroke to drain the response queue.
                    if let Ok(value) = &message {
                        if value.get("id").is_some() {
                            if let Some(method) = value["method"].as_str() {
                                if replies
                                    .send(server_response(
                                        value,
                                        method,
                                        &reader_settings,
                                        &reader_root,
                                    ))
                                    .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        }
                    }
                    // Ignore notifications (diagnostics/progress); navigation does not consume them.
                    if failed || message.as_ref().is_ok_and(|v| v.get("id").is_some()) {
                        if received.send(message).is_err() {
                            break;
                        }
                    }
                    if failed {
                        break;
                    }
                }
                reader_alive.store(false, Ordering::Relaxed);
            })
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.to_string());
        }
        let mut client = Self {
            child: Some(child),
            output,
            input,
            next_id: 0,
            capabilities: Value::Null,
            settings,
            alive,
        };
        let initialized = client.request(
            "initialize",
            json!({
                "processId": null, "clientInfo": {"name": "Araseo", "version": "0.1"},
                "rootUri": root_uri,
                "workspaceFolders": [{"uri":root_uri,"name":"project"}],
                "capabilities": {
                    "general": {"positionEncodings":["utf-16"]},
                "workspace":{"configuration":true,"workspaceFolders":true,
                    "didChangeWatchedFiles":{"dynamicRegistration":false},
                        "symbol":{"resolveSupport":{"properties":["location.range"]}}},
                    "textDocument":{"definition":{"linkSupport":true},
                        "documentSymbol":{"hierarchicalDocumentSymbolSupport":true},
                        "callHierarchy":{},"synchronization":{"dynamicRegistration":false}}
                }, "initializationOptions": options
            }),
            cancel,
            Duration::from_secs(45),
        )?;
        client.capabilities = initialized["capabilities"].clone();
        if client.capabilities["positionEncoding"]
            .as_str()
            .is_some_and(|encoding| encoding != "utf-16")
        {
            return Err("Language server did not accept UTF-16 positions".into());
        }
        client.notify("initialized", json!({}))?;
        client.notify(
            "workspace/didChangeConfiguration",
            json!({"settings":client.settings}),
        )?;
        Ok(client)
    }
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.enqueue(
            json!({"jsonrpc":"2.0","method":method,"params":params}),
            &Cancellation::default(),
        )
    }
    fn send(&self, value: Value) -> Result<()> {
        self.output
            .try_send(value)
            .map_err(|e| format!("Language server output queue unavailable: {e}"))
    }
    fn enqueue(&self, mut value: Value, cancel: &Cancellation) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            cancel.check()?;
            match self.output.try_send(value) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Full(unsent)) => {
                    value = unsent;
                    if Instant::now() >= deadline || !self.is_alive() {
                        return Err("Language server output queue unavailable".into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err("Language server output closed".into());
                }
            }
        }
    }
    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        cancel: &Cancellation,
        timeout: Duration,
    ) -> Result<Value> {
        cancel.check()?;
        self.next_id += 1;
        let id = self.next_id;
        self.enqueue(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            cancel,
        )?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Err(error) = cancel.check() {
                let _ = self.notify("$/cancelRequest", json!({"id":id}));
                return Err(error);
            }
            if Instant::now() >= deadline {
                let _ = self.notify("$/cancelRequest", json!({"id":id}));
                return Err(format!(
                    "Language server timed out ({method}); retry after indexing finishes"
                ));
            }
            match self.input.recv_timeout(Duration::from_millis(25)) {
                Ok(Ok(message)) => {
                    if message["id"].as_u64() == Some(id) {
                        if let Some(error) = message.get("error") {
                            return Err(format!(
                                "{}: {}",
                                method,
                                error["message"].as_str().unwrap_or("server error")
                            ));
                        }
                        return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                    }
                }
                Ok(Err(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Language server exited; retry to restart it".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
fn server_response(request: &Value, method: &str, settings: &Value, root_uri: &str) -> Value {
    let result = match method {
        "workspace/configuration" => {
            let items = request["params"]["items"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            Value::Array(
                items
                    .iter()
                    .map(|item| {
                        let mut value = settings;
                        for part in item["section"]
                            .as_str()
                            .unwrap_or("")
                            .split('.')
                            .filter(|p| !p.is_empty())
                        {
                            value = &value[part];
                        }
                        value.clone()
                    })
                    .collect(),
            )
        }
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability" => Value::Null,
        "workspace/workspaceFolders" => json!([{"uri":root_uri,"name":"project"}]),
        "workspace/applyEdit" => {
            json!({"applied":false,"failureReason":"Navigation client is read-only"})
        }
        _ => {
            return json!({"jsonrpc":"2.0","id":request["id"],
                "error":{"code":-32601,"message":"Unsupported client request"}});
        }
    };
    json!({"jsonrpc":"2.0","id":request["id"],"result":result})
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.send(json!({"jsonrpc":"2.0","method":"exit","params":null}));
        // Never wait for process termination on the UI thread.
        if let Some(mut child) = self.child.take() {
            let _ = std::thread::Builder::new()
                .name("araseo-lsp-reap".into())
                .spawn(move || {
                    let deadline = Instant::now() + Duration::from_millis(300);
                    loop {
                        if child.try_wait().ok().flatten().is_some() {
                            return;
                        }
                        if Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(15));
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_unicode_consecutive_messages_and_rejects_unbounded_input() {
        let value = json!({"result":"한글 😀"});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &value).unwrap();
        write_message(&mut bytes, &json!({"id":2})).unwrap();
        let mut input = std::io::Cursor::new(bytes);
        assert_eq!(read_message(&mut input).unwrap(), value);
        assert_eq!(read_message(&mut input).unwrap()["id"], 2);
        for invalid in [
            "Content-Length: 16777217\r\n\r\n",
            "Content-Length: 1\r\nContent-Length: 1\r\n\r\n0",
            "\r\n",
            "Content-Length: 7\r\n\r\n{}",
        ] {
            assert!(read_message(&mut std::io::Cursor::new(invalid)).is_err());
        }
        assert!(read_message(&mut std::io::Cursor::new("a".repeat(9000))).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn hung_requests_have_a_deadline_and_can_be_cancelled() {
        let script = r#"
import json,sys,time
while True:
    length=0
    while True:
        line=sys.stdin.buffer.readline()
        if not line: sys.exit()
        if line==b'\r\n': break
        if line.lower().startswith(b'content-length:'): length=int(line.split(b':')[1])
    message=json.loads(sys.stdin.buffer.read(length))
    if message.get('method')=='initialize':
        data=json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{'capabilities':{}}}).encode()
        sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n'%len(data)+data);sys.stdout.buffer.flush()
    elif message.get('id'): time.sleep(20)
"#;
        let mut command = Command::new("python3");
        command.args(["-u", "-c", script]);
        let cancel = Cancellation::default();
        let mut client =
            Client::spawn(&mut command, "file:///tmp", json!({}), json!({}), &cancel).unwrap();
        let now = Instant::now();
        assert!(
            client
                .request("hung", json!({}), &cancel, Duration::from_millis(80))
                .unwrap_err()
                .contains("timed out")
        );
        assert!(now.elapsed() < Duration::from_secs(1));
        let token = cancel.clone();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            token.cancel();
        });
        let now = Instant::now();
        assert!(
            client
                .request("hung", json!({}), &cancel, Duration::from_secs(10))
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(now.elapsed() < Duration::from_secs(1));
        release.join().unwrap();
    }
}
