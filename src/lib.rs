use serde_json::{Value, json};
use zed_extension_api::{
    self as zed,
    DebugAdapterBinary, DebugConfig, DebugRequest, DebugScenario,
    DebugTaskDefinition, Result, StartDebuggingRequestArguments,
    StartDebuggingRequestArgumentsRequest, Worktree, process::Command, resolve_tcp_template,
};

enum AdapterRuntime {
    /// dotnet <path> — for .NET Core / .NET 5+ managed assemblies (.dll)
    Dotnet,
    /// mono <path> — for Mono / .NET Framework assemblies (.exe)
    Mono,
}

struct UnityDebuggerExtension;

impl UnityDebuggerExtension {
    /// Returns (adapter_path, runtime). Detects runtime from file extension:
    /// `.exe` → Mono, anything else → dotnet.
    fn find_adapter(
        &self,
        user_provided_path: Option<String>,
        config: &str,
    ) -> Result<(String, AdapterRuntime), String> {
        let path = user_provided_path
            .filter(|p| !p.is_empty())
            .or_else(|| {
                serde_json::from_str::<Value>(config)
                    .ok()
                    .and_then(|cfg| cfg.get("adapterPath")?.as_str().map(str::to_string))
                    .filter(|p| !p.is_empty())
            })
            .ok_or_else(|| {
                "Adapter path not set. \
                 Add `\"adapterPath\": \"/path/to/adapter.dll\"` (or `.exe`) \
                 to your .zed/debug.json config."
                    .to_string()
            })?;

        let runtime = if path.ends_with(".exe") {
            AdapterRuntime::Mono
        } else {
            AdapterRuntime::Dotnet
        };

        Ok((path, runtime))
    }

    fn discover_unity_port(&self, worktree: &Worktree) -> Result<String, String> {
        let err = "Unity debugger not running. \
                   Make sure Unity Editor is open with Debug code optimization enabled \
                   (bottom toolbar toggle).";

        if worktree.which("lsof").is_some() {
            // macOS and Linux: list TCP ports in Unity's SDB range that are LISTEN
            let output = Command::new("/bin/sh")
                .args([
                    "-c",
                    "lsof -nP -i TCP:56000-56999 2>/dev/null \
                     | grep LISTEN | awk '{print $9}' | grep -oE '[0-9]+$' | head -1",
                ])
                .output()
                .map_err(|e| format!("Failed to run Unity port discovery: {}", e))?;
            let port = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !port.is_empty() {
                return Ok(format!("127.0.0.1:{}", port));
            }
        } else {
            // Windows: run netstat and filter in Rust to avoid shell quoting issues
            let output = Command::new("cmd")
                .args(["/c", "netstat -an"])
                .output()
                .map_err(|e| format!("Failed to run Unity port discovery: {}", e))?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                // Format: "  TCP    127.0.0.1:56716    0.0.0.0:0    LISTENING"
                if !line.contains("LISTENING") {
                    continue;
                }
                if let Some(port) = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|addr| addr.rsplit_once(':'))
                    .map(|(_, p)| p.trim())
                    .filter(|p| p.len() == 5 && p.starts_with("56"))
                {
                    return Ok(format!("127.0.0.1:{}", port));
                }
            }
        }

        Err(err.to_string())
    }

    fn build_config(
        &self,
        raw_config: &str,
        project_path: &str,
        endpoint: &str,
        is_mono: bool,
    ) -> String {
        let mut cfg: Value = serde_json::from_str(raw_config).unwrap_or_else(|_| json!({}));

        cfg["request"] = json!("attach");

        if is_mono {
            // unity-dap (net472/.exe) expects separate "address" and "port" fields
            if let Some((host, port_str)) = endpoint.rsplit_once(':') {
                cfg["address"] = json!(host);
                if let Ok(port_num) = port_str.parse::<u16>() {
                    cfg["port"] = json!(port_num);
                }
            }
            if let Some(obj) = cfg.as_object_mut() {
                obj.remove("projectPath");
                obj.remove("endPoint");
            }
        } else {
            // VSTU adapter (.dll) expects a combined "endPoint" string
            cfg["endPoint"] = json!(endpoint);
            if cfg.get("projectPath").is_none() || cfg["projectPath"] == json!(null) {
                cfg["projectPath"] = json!(project_path);
            }
        }

        if let Some(obj) = cfg.as_object_mut() {
            obj.remove("adapterPath");
            obj.remove("adapterArgs");
        }

        serde_json::to_string(&cfg).unwrap_or_else(|_| raw_config.to_string())
    }
}

impl zed::Extension for UnityDebuggerExtension {
    fn new() -> Self {
        Self
    }

    fn get_dap_binary(
        &mut self,
        _adapter_name: String,
        config: DebugTaskDefinition,
        user_provided_debug_adapter_path: Option<String>,
        worktree: &Worktree,
    ) -> Result<DebugAdapterBinary, String> {
        let (adapter_path, is_mono) = {
            let (path, runtime) =
                self.find_adapter(user_provided_debug_adapter_path, &config.config)?;
            let mono = matches!(runtime, AdapterRuntime::Mono);
            (path, mono)
        };

        // COMSPEC is set on Windows (points to cmd.exe) and not on Unix.
        let is_windows = worktree
            .shell_env()
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("COMSPEC"));

        // On Windows, net472 .exe files run natively under .NET Framework — no Mono needed.
        // Using Mono on Windows as a subprocess causes pipe/stdio issues.
        let (command, adapter_in_args) = if is_mono && is_windows {
            (adapter_path.clone(), false)
        } else if is_mono {
            let mono = worktree.which("mono").ok_or_else(|| {
                "Mono runtime (`mono`) not found on PATH. \
                 Install Mono from https://www.mono-project.com/download/stable/."
                    .to_string()
            })?;
            (mono, true)
        } else {
            let dotnet = worktree.which("dotnet").ok_or_else(|| {
                ".NET runtime (`dotnet`) not found on PATH. \
                 Install .NET 9 or later from https://dotnet.microsoft.com/download."
                    .to_string()
            })?;
            (dotnet, true)
        };

        let project_path = worktree.root_path();

        let raw: Value = serde_json::from_str(&config.config).unwrap_or_else(|_| json!({}));
        let endpoint = match raw.get("endPoint").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(ep) => ep.to_string(),
            None => self.discover_unity_port(worktree)?,
        };

        let adapter_config = self.build_config(&config.config, &project_path, &endpoint, is_mono);

        let connection = match config.tcp_connection {
            Some(template) => Some(resolve_tcp_template(template)?),
            None => None,
        };

        let raw_args: Vec<String> = raw
            .get("adapterArgs")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();

        let mut arguments = if adapter_in_args { vec![adapter_path] } else { vec![] };
        arguments.extend(raw_args);

        Ok(DebugAdapterBinary {
            command: Some(command),
            arguments,
            envs: vec![],
            cwd: None,
            connection,
            request_args: StartDebuggingRequestArguments {
                configuration: adapter_config,
                request: StartDebuggingRequestArgumentsRequest::Attach,
            },
        })
    }

    fn dap_request_kind(
        &mut self,
        _adapter_name: String,
        _config: Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        Ok(StartDebuggingRequestArgumentsRequest::Attach)
    }

    fn dap_config_to_scenario(&mut self, config: DebugConfig) -> Result<DebugScenario, String> {
        let project_path = match &config.request {
            DebugRequest::Launch(req) => req.cwd.clone().unwrap_or_default(),
            DebugRequest::Attach(_) => String::new(),
        };

        let adapter_config = json!({
            "request": "attach",
            "projectPath": if project_path.is_empty() { json!(null) } else { json!(project_path) },
        });

        Ok(DebugScenario {
            label: config.label,
            adapter: "unity".to_string(),
            config: serde_json::to_string(&adapter_config).map_err(|e| e.to_string())?,
            tcp_connection: None,
            build: None,
        })
    }
}

zed::register_extension!(UnityDebuggerExtension);
