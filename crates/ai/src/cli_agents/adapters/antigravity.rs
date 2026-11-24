//! Antigravity (`agy`). Flags are checked against `agy --help`.
//!
//! Antigravity runs prompts non-interactively with `--output-format stream-json -p <prompt>`.
//! In builder mode, `--dangerously-skip-permissions` is passed so tool requests proceed without prompting.
//! In blind visual review mode, Antigravity is locked down to the staged review directory via a
//! tool-restricted workspace custom agent (`--agent`), session-only `--settings` denying writes,
//! shell commands, URLs, and MCP tools with `allowNonWorkspaceAccess: false`, `--disable-slash-commands`,
//! and a workspace `PreToolUse` hook (`craft-anything-cli blind-review-guard`) that denies any tool
//! call other than reading direct staged files inside the review folder.

use super::*;

/// Name of the workspace custom agent staged inside the temporary blind-review folder.
pub(crate) const BLIND_REVIEW_AGENT: &str = "craft-anything-blind-critic";

/// Workspace custom agent definition restricting Antigravity's declared tools to read-only file
/// viewing (`view_file`), disabling subagents, command execution, MCP servers, skills, and plugins.
const BLIND_REVIEW_AGENT_MD: &str = "\
---\n\
name: craft-anything-blind-critic\n\
description: Code-blind visual reviewer limited to inspecting staged screenshots.\n\
tools:\n\
  - view_file\n\
mainAgent: true\n\
subagent: false\n\
commandExecutionPolicy: off\n\
mcpServers: []\n\
skills: []\n\
plugins: []\n\
---\n\
You are an independent, code-blind visual quality critic.\n\
Inspect only the staged image and diff files in the current directory and follow the user's review instructions strictly.\n";

/// Session-only `--settings` JSON enforcing default mode, `request-review` tool permissions,
/// no non-workspace access, disabled slash commands, and explicit deny rules for writes,
/// commands, URLs, and MCP tools (deny rules override any global or shared allow rules).
pub(crate) fn blind_review_settings() -> String {
    json!({
        "agentMode": "",
        "toolPermission": "request-review",
        "allowNonWorkspaceAccess": false,
        "disableSlashCommands": true,
        "permissions": {
            "allow": [],
            "ask": [],
            "deny": [
                "write_file(*)",
                "command(*)",
                "unsandboxed(*)",
                "read_url(*)",
                "execute_url(*)",
                "mcp(*)"
            ]
        }
    })
    .to_string()
}

fn blind_review_hook_command(cli: &Path, review_dir: &Path) -> String {
    #[cfg(windows)]
    {
        format!("\"{}\" blind-review-guard \"{}\"", cli.display(), review_dir.display())
    }
    #[cfg(not(windows))]
    {
        format!("{} blind-review-guard {}", crate::ssh::shell_quote(&cli.display().to_string()), crate::ssh::shell_quote(&review_dir.display().to_string()))
    }
}

pub(crate) fn prepare_blind_review_dir_with_cli(review_dir: &Path, cli: &Path) -> Result<(), ToolError> {
    let agents_root = review_dir.join(".agents");
    let agent_dir = agents_root.join("agents").join(BLIND_REVIEW_AGENT);
    std::fs::create_dir_all(&agent_dir).map_err(|e| ToolError::io(agent_dir.display(), e))?;
    let agent_md = agent_dir.join("agent.md");
    std::fs::write(&agent_md, BLIND_REVIEW_AGENT_MD).map_err(|e| ToolError::io(agent_md.display(), e))?;

    let hooks = json!({
        "craft-anything-blind-guard": {
            "PreToolUse": [
                {
                    "matcher": "*",
                    "hooks": [
                        {
                            "type": "command",
                            "command": blind_review_hook_command(cli, review_dir),
                            "timeout": 10
                        }
                    ]
                }
            ]
        }
    });
    let hooks_bytes = serde_json::to_vec_pretty(&hooks).map_err(|e| ToolError::internal(e.to_string()))?;
    let hooks_path = agents_root.join("hooks.json");
    std::fs::write(&hooks_path, hooks_bytes).map_err(|e| ToolError::io(hooks_path.display(), e))?;
    Ok(())
}

/// Parse `agy models` tab-separated output (`<slug>\t<display name>`) into model slugs.
fn antigravity_models(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((slug, _label)) = line.split_once('\t') else { continue };
        let slug = slug.trim();
        if !slug.is_empty() && !slug.chars().any(char::is_whitespace) && !out.iter().any(|s| s == slug) {
            out.push(slug.to_string());
        }
    }
    out
}

pub struct Antigravity;

impl AgentAdapter for Antigravity {
    fn ids(&self) -> &'static [&'static str] {
        &["antigravity", "agy"]
    }

    fn supports_blind_review(&self) -> bool {
        true
    }

    fn model_in_use(&self, options: Option<&BackendOptions>) -> Option<String> {
        Some(options.and_then(|o| o.cli_model.clone()).unwrap_or_else(|| "Antigravity default".into()))
    }

    /// Antigravity's own model list (`agy models`), after its own default.
    fn model_choices(&self, config: &BackendConfig, options: &BackendOptions) -> Result<Vec<String>, ToolError> {
        let program = super::program_of(config, options)?;
        let ask = [program, "models".into()];
        let p = match config.ssh_host.as_deref().filter(|h| !h.trim().is_empty()) {
            Some(h) => Process::new("ssh").args(crate::ssh::ssh_args(h, &ask)),
            None => Process::new(&ask[0]).args(&ask[1..]),
        };
        let out = p.timeout(Duration::from_secs(15)).stdout_cap(2 * 1024 * 1024).run_checked()?;
        let mut models = vec![crate::anthropic::CLAUDE_CODE_DEFAULT.to_string()];
        models.extend(antigravity_models(&out.stdout_text()));
        Ok(models)
    }

    fn offers_own_default(&self) -> bool {
        true
    }

    fn blind_review_required_flags(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (&["--help"], &["--settings", "--agent", "--disable-slash-commands", "--output-format", "stream-json"])
    }

    fn prepare_blind_review_dir(&self, review_dir: &Path) -> Result<(), ToolError> {
        let cli = craft_anything_platform::self_cli_path().ok_or_else(|| {
            ToolError::new(ErrorCode::EngineMissing, "Couldn't find `craft-anything-cli` to enforce Antigravity's read-only blind-review hook.")
                .with_remediation("Build or install `craft-anything-cli` next to Craft Anything (or on PATH), or choose another reviewer.")
        })?;
        prepare_blind_review_dir_with_cli(review_dir, &cli)
    }

    fn blind_review_args(&self, cx: &ArgsCx) -> Vec<String> {
        // Deliberately omit `cx.options.args` (which may carry `--add-dir`, `--mode`,
        // `--settings`, `--dangerously-skip-permissions`, or `--remote-agent`) and any
        // `--conversation` resume id so every blind review runs statelessly inside the
        // staged folder with read-only, tool-restricted settings.
        let mut a = vec!["--settings".into(), blind_review_settings(), "--agent".into(), BLIND_REVIEW_AGENT.into(), "--disable-slash-commands".into()];
        if let Some(m) = cx.options.cli_model.as_deref().filter(|m| !m.trim().is_empty()) {
            a.extend(["--model".into(), m.to_string()]);
        }
        a.extend(["--output-format".into(), "stream-json".into(), "-p".into(), format!("{}{}", cx.preamble, cx.prompt)]);
        a
    }

    fn args(&self, cx: &ArgsCx) -> Vec<String> {
        let mut a = cx.options.args.clone();
        if cx.builder {
            a.push("--dangerously-skip-permissions".into());
        }
        if let Some(m) = &cx.options.cli_model {
            a.extend(["--model".into(), m.clone()]);
        }
        if let Some(r) = cx.resume {
            a.extend(["--conversation".into(), r.to_string()]);
        }
        a.extend(["--output-format".into(), "stream-json".into(), "-p".into(), format!("{}{}", cx.preamble, cx.prompt)]);
        a
    }

    fn required_flags(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (&["--help"], &["--output-format", "stream-json"])
    }

    fn streams_json(&self) -> bool {
        true
    }

    fn resumes(&self) -> bool {
        true
    }

    fn parse(&self, p: &mut StreamParser, v: &Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let event = v.get("event").and_then(Value::as_str).unwrap_or_default();
        match event {
            "init" => {
                if let Some(id) = v.get("conversation_id").and_then(Value::as_str) {
                    p.session_id = Some(id.to_string());
                }
            }
            "step_update" => {
                let Some(su) = v.get("step_update") else { return out };
                if let Some(id) = su.get("conversation_id").and_then(Value::as_str) {
                    p.session_id.get_or_insert_with(|| id.to_string());
                }
                let step_type = su.get("step_type").and_then(Value::as_str).unwrap_or_default();
                let state = su.get("state").and_then(Value::as_str).unwrap_or_default();
                match step_type {
                    "agent_response" => {
                        if let Some(text) = su.get("text_delta").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            out.push(AgentEvent::TextDelta { text: text.to_string() });
                        }
                    }
                    "tool" => {
                        let id = su.get("step_index").map(|i| i.to_string()).unwrap_or_default();
                        let name = su.get("tool_name").or_else(|| su.pointer("/tool_info/name")).and_then(Value::as_str).unwrap_or_default();
                        if state == "ACTIVE" {
                            let input = su.pointer("/tool_info/parameters").cloned().unwrap_or(Value::Null);
                            out.push(p.started(&id, name, input));
                        } else if state == "DONE" || state == "ERROR" {
                            if !p.tool_names.contains_key(&id) {
                                let input = su.pointer("/tool_info/parameters").cloned().unwrap_or(Value::Null);
                                out.push(p.started(&id, name, input));
                            }
                            let ok = state != "ERROR" && su.pointer("/tool_info/error").is_none();
                            let text = su.pointer("/tool_info/output").or_else(|| su.pointer("/tool_info/error")).map(content_text).unwrap_or_default();
                            out.push(p.finished(&id, ok, &text));
                        }
                    }
                    _ => {}
                }
            }
            "result" => {
                p.saw_result = true;
                if let Some(res) = v.get("result") {
                    if let Some(u) = res.get("usage") {
                        let in_tok = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                        let cache_tok = u.get("cache_read_tokens").and_then(Value::as_u64).unwrap_or(0);
                        p.input_tokens = in_tok.saturating_add(cache_tok);
                        let out_tok = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                        let think_tok = u.get("thinking_tokens").and_then(Value::as_u64).unwrap_or(0);
                        p.output_tokens = out_tok.saturating_add(think_tok);
                    }
                    if res.get("status").and_then(Value::as_str) == Some("ERROR") {
                        let reply = res.get("response").and_then(Value::as_str).unwrap_or_default();
                        let why = res.get("error").and_then(Value::as_str).unwrap_or("run failed");
                        if reply.trim().is_empty() {
                            out.push(AgentEvent::Error { message: format!("Antigravity: {why}"), remediation: None, log: None });
                        } else {
                            // Antigravity keeps reporting a conversation's old error (its `error_id`
                            // names a step from an earlier turn) on every later result, even after a
                            // turn answers in full. A reply that came back ends the turn as done;
                            // the words stay visible without failing work that succeeded.
                            out.push(AgentEvent::Status {
                                text: format!("Antigravity replied, yet still reports an earlier error of the conversation: {why}"),
                            });
                        }
                    }
                }
            }
            "error" => {
                let msg = v.pointer("/error/message").or_else(|| v.get("error")).and_then(Value::as_str).unwrap_or("unknown error");
                out.push(AgentEvent::Error { message: format!("Antigravity: {msg}"), remediation: None, log: None });
            }
            _ => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(builder: bool, resume: Option<&str>, options: &BackendOptions) -> Vec<String> {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        Antigravity.args(&ArgsCx {
            options,
            prompt: "analyse repo",
            preamble: "PRE ",
            mcp: &mcp,
            resume,
            builder,
            reads_checked: false,
            question: false,
            message_file: None,
        })
    }

    #[test]
    fn antigravity_args_construction() {
        let default_opts = BackendOptions::default();
        assert_eq!(args(false, None, &default_opts), ["--output-format", "stream-json", "-p", "PRE analyse repo"]);

        let builder_opts =
            BackendOptions { cli_model: Some("Gemini 3.8 Flash (Medium)".into()), args: vec!["--add-dir".into(), "/repo".into()], ..Default::default() };
        assert_eq!(
            args(true, Some("conv-123"), &builder_opts),
            [
                "--add-dir",
                "/repo",
                "--dangerously-skip-permissions",
                "--model",
                "Gemini 3.8 Flash (Medium)",
                "--conversation",
                "conv-123",
                "--output-format",
                "stream-json",
                "-p",
                "PRE analyse repo"
            ]
        );
        assert!(Antigravity.resumes());
        assert_eq!(Antigravity.required_flags(), (&["--help"][..], &["--output-format", "stream-json"][..]));
        assert!(AgentAdapters::built_in().for_agent("antigravity").ids().contains(&"antigravity"));
        assert!(AgentAdapters::built_in().for_agent("agy").ids().contains(&"agy"));
    }

    #[test]
    fn antigravity_stream_parsing() {
        let adapter: Arc<dyn AgentAdapter> = Arc::new(Antigravity);
        let mut p = StreamParser::new(adapter);

        let init = json!({
            "event": "init",
            "conversation_id": "conv-456",
            "init": {"cwd": "/tmp", "permission_mode": "always-proceed"}
        });
        assert!(p.line(&init.to_string()).is_empty());
        assert_eq!(p.session_id.as_deref(), Some("conv-456"));

        let text_step = json!({
            "event": "step_update",
            "step_update": {
                "conversation_id": "conv-456",
                "step_index": 1,
                "state": "ACTIVE",
                "step_type": "agent_response",
                "text_delta": "Hello from Antigravity!"
            }
        });
        let evs = p.line(&text_step.to_string());
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::TextDelta { text } => assert_eq!(text, "Hello from Antigravity!"),
            other => panic!("expected TextDelta, got {other:?}"),
        }

        let tool_active = json!({
            "event": "step_update",
            "step_update": {
                "step_index": 2,
                "state": "ACTIVE",
                "step_type": "tool",
                "tool_name": "run_command",
                "tool_info": {
                    "name": "run_command",
                    "parameters": {"CommandLine": "cargo check"}
                }
            }
        });
        let evs = p.line(&tool_active.to_string());
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::ToolStarted { call_id, tool, input } => {
                assert_eq!(call_id, "2");
                assert_eq!(tool, "run_command");
                assert_eq!(input["CommandLine"], "cargo check");
            }
            other => panic!("expected ToolStarted, got {other:?}"),
        }

        let tool_done = json!({
            "event": "step_update",
            "step_update": {
                "step_index": 2,
                "state": "DONE",
                "step_type": "tool",
                "tool_name": "run_command",
                "tool_info": {
                    "name": "run_command",
                    "parameters": {"CommandLine": "cargo check"},
                    "output": "Finished dev profile"
                }
            }
        });
        let evs = p.line(&tool_done.to_string());
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::ToolFinished { call_id, tool, ok, summary, .. } => {
                assert_eq!(call_id, "2");
                assert_eq!(tool, "run_command");
                assert!(ok);
                assert_eq!(summary, "Finished dev profile");
            }
            other => panic!("expected ToolFinished, got {other:?}"),
        }

        let result = json!({
            "event": "result",
            "result": {
                "conversation_id": "conv-456",
                "status": "SUCCESS",
                "response": "Done!",
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "thinking_tokens": 10,
                    "cache_read_tokens": 20,
                    "total_tokens": 180
                }
            }
        });
        let evs = p.line(&result.to_string());
        assert!(evs.is_empty());
        assert!(p.saw_result);
        assert_eq!(p.input_tokens, 120);
        assert_eq!(p.output_tokens, 60);

        let err = json!({
            "event": "error",
            "error": "authentication expired"
        });
        let evs = p.line(&err.to_string());
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Error { message, .. } => assert!(message.contains("authentication expired")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn an_error_result_that_carries_a_reply_keeps_the_turn_alive() {
        let adapter: Arc<dyn AgentAdapter> = Arc::new(Antigravity);
        let mut p = StreamParser::new(adapter);
        let result = json!({
            "event": "result",
            "result": {
                "conversation_id": "conv-456",
                "status": "ERROR",
                "response": "I rebuilt the app and committed.\n",
                "error": "API error (attempt 2): request failed: stream idle timeout (5m0s): context deadline exceeded",
                "usage": {"input_tokens": 10, "output_tokens": 5, "thinking_tokens": 0, "cache_read_tokens": 0, "total_tokens": 15}
            }
        });
        let evs = p.line(&result.to_string());
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Status { text } => assert!(text.contains("stream idle timeout"), "the stale error stays visible: {text}"),
            other => panic!("expected Status, got {other:?}"),
        }
        let no_reply = json!({
            "event": "result",
            "result": {"status": "ERROR", "response": "", "error": "Eligibility check failed: EOF"}
        });
        let evs = p.line(&no_reply.to_string());
        assert!(matches!(evs[0], AgentEvent::Error { .. }), "an error with nothing delivered still fails the turn");
    }

    #[test]
    fn blind_review_prepares_agent_and_hook_in_review_dir() {
        let d = tempfile::tempdir().unwrap();
        let review_dir = d.path().join("review folder");
        std::fs::create_dir_all(&review_dir).unwrap();
        let cli = d.path().join("my bin").join("craft-anything-cli");

        prepare_blind_review_dir_with_cli(&review_dir, &cli).unwrap();

        let agent_md = std::fs::read_to_string(review_dir.join(".agents").join("agents").join(BLIND_REVIEW_AGENT).join("agent.md")).unwrap();
        assert!(agent_md.contains("name: craft-anything-blind-critic") && agent_md.contains("- view_file"), "{agent_md}");
        assert!(agent_md.contains("subagent: false") && agent_md.contains("commandExecutionPolicy: off"), "{agent_md}");

        let hooks: Value = serde_json::from_slice(&std::fs::read(review_dir.join(".agents").join("hooks.json")).unwrap()).unwrap();
        let entry = &hooks["craft-anything-blind-guard"]["PreToolUse"][0];
        assert_eq!(entry["matcher"], "*");
        let cmd = entry["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("blind-review-guard"), "{cmd}");
        assert!(cmd.contains("craft-anything-cli") && cmd.contains("review folder"), "{cmd}");

        assert!(Antigravity.supports_blind_review());
        let (help, needed) = Antigravity.blind_review_required_flags();
        assert_eq!(help, &["--help"]);
        for flag in ["--settings", "--agent", "--disable-slash-commands", "--output-format", "stream-json"] {
            assert!(needed.contains(&flag), "missing required blind-review flag {flag}");
        }
    }

    #[test]
    fn antigravity_offers_the_models_its_cli_lists() {
        let out = "\
Fetching available models...
gemini-4-neon-high\tGemini 4 Neon (High)
gemini-3.8-flash-medium\tGemini 3.8 Flash (Medium)
gemini-4-neon-high\tGemini 4 Neon (High)
bad line without tab
has space\tInvalid
\tMissing Slug
claude-sonnet-5-5-high\tClaude Sonnet 5.5 (High)
";
        assert_eq!(antigravity_models(out), ["gemini-4-neon-high", "gemini-3.8-flash-medium", "claude-sonnet-5-5-high"]);
        assert!(Antigravity.offers_own_default());
        assert_eq!(Antigravity.model_in_use(None).as_deref(), Some("Antigravity default"));
        let chosen = BackendOptions { cli_model: Some("gemini-3.8-flash-medium".into()), ..Default::default() };
        assert_eq!(Antigravity.model_in_use(Some(&chosen)).as_deref(), Some("gemini-3.8-flash-medium"));
    }
}
