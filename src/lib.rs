use serde_json::{Value, json};
use zed_extension_api::{
    self as zed,
    DebugAdapterBinary, DebugConfig, DebugRequest, DebugScenario,
    DebugTaskDefinition, Result, StartDebuggingRequestArguments,
    StartDebuggingRequestArgumentsRequest, Worktree, process::Command, resolve_tcp_template,
};

struct UnityDebuggerExtension;

impl UnityDebuggerExtension {
    fn find_adapter_dll(
        &self,
        user_provided_path: Option<String>,
        config: &str,
    ) -> Result<String, String> {
        if let Some(path) = user_provided_path {
            if !path.is_empty() {
                return Ok(path);
            }
        }

        if let Ok(cfg) = serde_json::from_str::<Value>(config) {
            if let Some(path) = cfg.get("adapterPath").and_then(|v| v.as_str()) {
                if !path.is_empty() {
                    return Ok(path.to_string());
                }
            }
        }

        Err(
            "UnityDebugAdapter.dll path not set. \
             Add `\"adapterPath\": \"/path/to/UnityDebugAdapter.dll\"` to your \
             .zed/debug.json config."
                .to_string(),
        )
    }

    fn discover_unity_port(&self) -> Result<String, String> {
        let output = Command::new("/bin/sh")
            .args([
                "-c",
                "lsof -i | grep Unity | grep -E '56[0-9]{3}' | awk '{print $9}' | grep -oE '56[0-9]{3}$'",
            ])
            .output()
            .map_err(|e| format!("Failed to run Unity port discovery: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let port = stdout.lines().next().map(str::trim).unwrap_or("").to_string();

        if port.is_empty() {
            return Err(
                "Unity debugger not running. \
                 Make sure Unity Editor is open with Debug code optimization enabled \
                 (bottom toolbar toggle)."
                    .to_string(),
            );
        }

        Ok(format!("127.0.0.1:{}", port))
    }

    fn build_config(&self, raw_config: &str, project_path: &str, endpoint: &str) -> String {
        let mut cfg: Value = serde_json::from_str(raw_config).unwrap_or_else(|_| json!({}));

        cfg["request"] = json!("attach");
        cfg["endPoint"] = json!(endpoint);

        if cfg.get("projectPath").is_none() || cfg["projectPath"] == json!(null) {
            cfg["projectPath"] = json!(project_path);
        }

        if let Some(obj) = cfg.as_object_mut() {
            obj.remove("adapterPath");
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
        let dotnet = worktree.which("dotnet").ok_or_else(|| {
            ".NET runtime (`dotnet`) not found on PATH. \
             Install .NET 9 or later from https://dotnet.microsoft.com/download."
                .to_string()
        })?;

        let dll_path = self.find_adapter_dll(user_provided_debug_adapter_path, &config.config)?;
        let project_path = worktree.root_path();

        let raw: Value = serde_json::from_str(&config.config).unwrap_or_else(|_| json!({}));
        let endpoint = match raw.get("endPoint").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(ep) => ep.to_string(),
            None => self.discover_unity_port()?,
        };

        let adapter_config = self.build_config(&config.config, &project_path, &endpoint);

        let connection = match config.tcp_connection {
            Some(template) => Some(resolve_tcp_template(template)?),
            None => None,
        };

        Ok(DebugAdapterBinary {
            command: Some(dotnet),
            arguments: vec![dll_path],
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
