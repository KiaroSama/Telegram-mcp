use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};

pub const DEADLINE: Duration = Duration::from_secs(20);
pub fn stderr_diagnostic(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    for pattern in [
        r"(?s)-----BEGIN [^-]*PRIVATE KEY-----.*?(?:-----END [^-]*PRIVATE KEY-----|\z)",
        r"(?i)\bbearer\s+[A-Za-z0-9_+./=-]+",
        r#"(?i)(?:token|auth_key|api_hash|api_key|password|secret|private_key|session_string)["']?\s*[:=]\s*["']?[^\s"',;\]}]+"#,
        r"\b[0-9a-fA-F]{32,}\b",
        r"[A-Za-z0-9_+/=-]{96,}",
    ] {
        text = regex::Regex::new(pattern)
            .unwrap()
            .replace_all(&text, "[REDACTED]")
            .into_owned();
    }
    text.chars().take(4096).collect()
}
pub fn catalogue_names() -> std::collections::BTreeSet<String> {
    let descriptors: Vec<Value> =
        serde_json::from_str(include_str!("../../resources/tool-catalogue.json")).unwrap();
    descriptors
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

pub struct Fixture {
    pub _temp: TempDir,
    pub cwd: PathBuf,
    pub state: PathBuf,
    pub files: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("runtime حساب");
        std::fs::create_dir(&cwd).unwrap();
        let files = cwd.join("files");
        std::fs::create_dir(&files).unwrap();
        let state = temp.path().join("private-state");
        std::fs::write(cwd.join(".env"), "# Synthetic offline transport fixture\n").unwrap();
        Self {
            _temp: temp,
            cwd,
            state,
            files,
        }
    }
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_telegram-mcp"));
        command.args(args).current_dir(&self.cwd).kill_on_drop(true);
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("TELEGRAM_") || name.starts_with("MCP_") {
                command.env_remove(key);
            }
        }
        command
            .env("TELEGRAM_STATE_DIR", &self.state)
            .env(
                "TELEGRAM_FILE_ROOTS",
                serde_json::to_string(&[self.files.to_string_lossy()]).unwrap(),
            )
            .env("MCP_TRANSPORT", "stdio")
            .env("TOKIO_WORKER_THREADS", "2");
        command
    }
    pub fn env_file(&self, text: &str) {
        std::fs::write(self.cwd.join(".env"), text.as_bytes()).unwrap();
    }
    pub fn assert_no_telegram_sessions(&self) {
        if !self.state.exists() {
            return;
        }
        for entry in std::fs::read_dir(&self.state).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !name.ends_with(".session.json"),
                "A local transport operation unexpectedly created a Telegram authorization"
            );
            assert!(
                !name.starts_with("auth-"),
                "A local transport operation unexpectedly acquired a Telegram authorization"
            );
        }
    }
    pub fn assert_logs_omit(&self, values: &[&str]) {
        if !self.state.exists() {
            return;
        }
        for entry in std::fs::read_dir(&self.state).unwrap() {
            let entry = entry.unwrap();
            if !entry.file_name().to_string_lossy().contains(".log") {
                continue;
            }
            let text = std::fs::read_to_string(entry.path()).unwrap();
            for value in values {
                assert!(
                    !text.contains(value),
                    "A caller-controlled credential-like value reached the runtime log"
                );
            }
        }
    }
}

pub struct McpProcess {
    child: Child,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
    stderr: JoinHandle<Vec<u8>>,
}
impl McpProcess {
    pub fn start(mut command: Command) -> Self {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut stderr = child.stderr.take().unwrap();
        let stderr = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stderr.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                let retained = count.min((16usize * 1024).saturating_sub(bytes.len()));
                bytes.extend_from_slice(&buffer[..retained]);
            }
            bytes
        });
        Self {
            child,
            input,
            output,
            stderr,
        }
    }
    pub async fn send(&mut self, value: &Value) {
        let mut data = serde_json::to_vec(value).unwrap();
        data.push(b'\n');
        let input = self.input.as_mut().unwrap();
        input.write_all(&data).await.unwrap();
        input.flush().await.unwrap();
    }
    pub async fn response(&mut self, id: i64) -> Value {
        for _ in 0..64 {
            let line = tokio::time::timeout(DEADLINE, self.output.next_line())
                .await
                .expect("MCP response exceeded deadline")
                .expect("MCP stdout read failed");
            let Some(line) = line else {
                let diagnostic = match tokio::time::timeout(DEADLINE, &mut self.stderr).await {
                    Ok(Ok(bytes)) => stderr_diagnostic(&bytes),
                    Ok(Err(_)) => "stderr collector failed".into(),
                    Err(_) => "stderr did not close within the diagnostic deadline".into(),
                };
                panic!(
                    "MCP child closed stdout before responding to request {id}; bounded child stderr: {diagnostic}"
                );
            };
            assert!(
                line.len() < 8 * 1024 * 1024,
                "MCP frame exceeded its response budget"
            );
            let value: Value = serde_json::from_str(&line)
                .expect("MCP stdout must contain only JSON-RPC frames, never runtime logs");
            assert_eq!(value["jsonrpc"], "2.0");
            if value.get("id") == Some(&json!(id)) {
                return value;
            }
            assert!(
                value.get("id").is_none() && value.get("method").is_some(),
                "Unexpected JSON-RPC response or non-protocol output"
            );
        }
        panic!("MCP child emitted too many unsolicited frames")
    }
    pub async fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        self.response(id).await
    }
    pub async fn initialize(&mut self, version: &str) {
        let result=self.request(1,"initialize",json!({"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"native-process-test","version":"1.0"}})).await;
        assert_eq!(
            result["result"]["protocolVersion"], version,
            "The supported legacy protocol must be negotiated accurately"
        );
        assert!(result["result"]["capabilities"]["tools"].is_object());
        self.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
    }
    pub async fn finish(mut self) -> String {
        drop(self.input.take());
        let status = tokio::time::timeout(DEADLINE, self.child.wait())
            .await
            .expect("MCP child did not exit after stdin EOF")
            .unwrap();
        assert!(
            status.success(),
            "MCP child failed during clean stdio shutdown"
        );
        tokio::time::timeout(DEADLINE, async {
            while let Some(line) = self.output.next_line().await.unwrap() {
                let value: Value = serde_json::from_str(&line)
                    .expect("MCP stdout contained non-JSON output during shutdown");
                assert_eq!(value["jsonrpc"], "2.0");
            }
        })
        .await
        .expect("MCP stdout did not reach EOF after its process exited; a child may have inherited the client pipe");
        let stderr = tokio::time::timeout(DEADLINE, self.stderr)
            .await
            .expect("MCP stderr did not reach EOF after its process exited; a child may have inherited the client pipe")
            .unwrap();
        String::from_utf8(stderr).expect("MCP stderr must be UTF-8")
    }
}

pub fn latest_meta(mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":rmcp::model::ProtocolVersion::LATEST.as_str(),"io.modelcontextprotocol/clientInfo":{"name":"native-inline-test","version":"1.0"},"io.modelcontextprotocol/clientCapabilities":{}});
    params
}
pub fn tools(response: &Value) -> &[Value] {
    response["result"]["tools"]
        .as_array()
        .expect("tools/list must return its actual tool array")
}
pub fn assert_catalogue(rows: &[Value]) {
    let expected = catalogue_names();
    assert_eq!(rows.len(), expected.len());
    let mut names = std::collections::HashSet::new();
    for tool in rows {
        let name = tool["name"].as_str().unwrap();
        assert!(names.insert(name), "Duplicate exposed tool");
        assert!(!tool["description"].as_str().unwrap().trim().is_empty());
        assert_eq!(tool["inputSchema"]["type"], "object");
        for field in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(
                tool["annotations"][field].is_boolean(),
                "A tool is missing a public safety annotation"
            );
        }
    }
    assert_eq!(
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    let accounts = rows
        .iter()
        .find(|tool| tool["name"] == "list_accounts")
        .unwrap();
    assert_eq!(accounts["annotations"]["readOnlyHint"], true);
    assert_eq!(accounts["inputSchema"]["additionalProperties"], false);
    assert!(names.contains("send_message"));
    assert!(names.contains("get_privacy_settings"));
    assert!(names.contains("secret_chat_status"));
    assert!(names.contains("send_secret_message"));
}
pub fn text_result(response: &Value) -> Value {
    assert_ne!(
        response["result"]["isError"], true,
        "Local offline tool unexpectedly failed"
    );
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("MCP tool response must contain text content");
    serde_json::from_str(text).expect("Tool text must preserve its structured JSON result")
}
pub async fn cli(fixture: &Fixture, args: &[&str], input: Option<&[u8]>) -> std::process::Output {
    let mut command = fixture.command(args);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).await.unwrap();
    } else {
        drop(child.stdin.take());
    }
    tokio::time::timeout(DEADLINE, child.wait_with_output())
        .await
        .expect("CLI command exceeded its deadline")
        .unwrap()
}
