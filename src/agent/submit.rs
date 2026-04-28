use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use super::agentic_loop::agentic_loop;
use super::models::fetch_models_for_provider;
use super::project_tree::{build_project_tree, build_structural_map};
use super::provider::ProviderKind;
use super::{
    append_session_start_record, AgentPanel, AgentStatus, ChatMessage, ContextBreakdown, Role,
    StreamEvent, SubmitCtx,
};

impl AgentPanel {
    /// Submit input, launching the agentic tool-calling loop in the background.
    #[allow(clippy::too_many_arguments)]
    pub async fn submit(
        &mut self,
        context: Option<String>,
        project_root: PathBuf,
        max_rounds: usize,
        warning_threshold: usize,
        preferred_model: &str,
        auto_compress: bool,
        observation_mask_threshold_chars: usize,
        expand_threshold_chars: usize,
    ) -> Result<()> {
        if !self.has_pending_content() {
            return Ok(());
        }

        // Record the conversation start time on the first submit.
        if self.conversation.session_start_secs == 0 {
            self.conversation.session_start_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
        }

        // Save non-empty input to history before consuming it.
        let trimmed = self.conversation.input.trim().to_string();
        if !trimmed.is_empty() {
            self.conversation.input_history.push(trimmed);
            if self.conversation.input_history.len() > 50 {
                self.conversation.input_history.remove(0);
            }
        }
        self.conversation.input_cursor = 0;
        self.conversation.history_idx = None;
        self.conversation.input_saved.clear();

        let typed_text = std::mem::take(&mut self.conversation.input);
        let pasted = std::mem::take(&mut self.pasted_blocks);
        let files = std::mem::take(&mut self.file_blocks);
        let images = std::mem::take(&mut self.image_blocks);

        // Resolve context window early so the file-block cap below can use it.
        let context_limit = self.context_window_size();

        // Assemble user text: file blocks first (structured context), then pasted
        // blocks (ad-hoc snippets), then typed input.  Each section separated by \n\n.
        //
        // Cap total injected file content so the prompt never exceeds the model
        // window.  Each file is already truncated to AT_PICKER_MAX_LINES lines by the
        // picker; this enforces an aggregate ceiling.  Budget = 50% of the context
        // window, leaving room for the system prompt, history, and typed instruction.
        let max_file_tokens: usize = (context_limit as usize) / 2;
        let mut used_file_tokens: usize = 0;
        let mut parts: Vec<String> = Vec::new();
        for (name, content, _) in &files {
            let file_tokens = content.len() / 4;
            if used_file_tokens >= max_file_tokens {
                parts.push(format!(
                    "File: {name}\n\n[omitted — aggregate file context limit \
                     ({max_file_tokens} tokens) reached; use read_file to access this file]"
                ));
                continue;
            }
            // Use 4-backtick fences so that any ``` sequences inside the file
            // (e.g. code blocks in a .md file) cannot prematurely close the outer
            // fence and break both the model's context and the UI renderer.
            let lang = std::path::Path::new(name.as_str())
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let remaining_chars = (max_file_tokens - used_file_tokens) * 4;
            if content.len() > remaining_chars {
                let truncated: String = content
                    .lines()
                    .scan(0usize, |acc, line| {
                        *acc += line.len() + 1;
                        Some((*acc, line))
                    })
                    .take_while(|(acc, _)| *acc <= remaining_chars)
                    .map(|(_, line)| line)
                    .collect::<Vec<_>>()
                    .join("\n");
                used_file_tokens += truncated.len() / 4;
                parts.push(format!(
                    "File: {name}\n\n````{lang}\n{truncated}\n````\n\
                     [truncated — aggregate file context limit reached; \
                     use read_file for the rest]"
                ));
            } else {
                used_file_tokens += file_tokens;
                parts.push(format!("File: {name}\n\n````{lang}\n{content}\n````"));
            }
        }
        if used_file_tokens >= max_file_tokens {
            warn!(
                "[ctx] File block budget ({max_file_tokens}t) exhausted — \
                 some attached files were omitted or truncated. \
                 Select fewer files or use read_file in your instruction."
            );
        }
        for (text, _) in &pasted {
            parts.push(text.clone());
        }
        let user_text = if parts.is_empty() {
            typed_text
        } else {
            let mut combined = parts.join("\n\n");
            if !typed_text.trim().is_empty() {
                combined.push_str("\n\n");
                combined.push_str(&typed_text);
            }
            combined
        };
        // Slash-command interception: if a prompt framework is active and the user
        // typed "/<command> [context]", resolve the template and rebuild the message.
        // The template becomes the structured instruction; any trailing text is
        // appended as "user context" and the combined string is sent as the user turn.
        //
        // When the command sets clears_context (all built-in spec-kit phases do),
        // a new conversation is started automatically so the phase template runs
        // against a clean context window — no prior-phase history to re-send.
        //
        // Resolve into owned Strings first so the immutable borrow on self.spec_framework
        // is released before the potential mutable new_conversation() call.
        //
        // Phase 2 (ADR 0100): capture the raw command name and feature arg before
        // template expansion so the SpecSlicer can inject a virtual context block
        // for speckit.implement and speckit.tasks without reparsing user_text.
        let spec_cmd_ctx: Option<(String, String)> =
            user_text.trim_start().strip_prefix('/').and_then(|s| {
                let cmd = s.split_whitespace().next()?;
                if cmd.starts_with("openspec.") {
                    let rest = s[cmd.len()..].trim_start().to_string();
                    Some((cmd.to_string(), rest))
                } else {
                    None
                }
            });
        let resolved: Option<(String, String, bool)> =
            self.spec_framework.as_ref().and_then(|fw| {
                fw.resolve(&user_text)
                    .map(|(tmpl, rest, clears)| (tmpl.to_string(), rest.to_string(), clears))
            });
        let user_text = if let Some((template, rest, clears_context)) = resolved {
            if clears_context {
                let model_display = self.selected_model_display().to_string();
                let cmd =
                    user_text.trim_start_matches('/').split_whitespace().next().unwrap_or("?");
                info!("[spec] auto-clearing conversation before /{cmd} (clears_context = true)");
                self.new_conversation(&model_display);
            }
            // Append whatever the user typed after the command as context.
            if rest.is_empty() {
                template
            } else {
                format!("{template}{rest}")
            }
        } else {
            user_text
        };
        // Phase 2 — SpecSlicer (ADR 0100): for openspec.apply, inject a pre-extracted
        // virtual context block (active task + relevant design sections) to save the
        // model a full-file read round on every apply turn.
        let user_text =
            if matches!(spec_cmd_ctx.as_ref().map(|(c, _)| c.as_str()), Some("openspec.apply")) {
                let change = spec_cmd_ctx
                    .as_ref()
                    .and_then(|(_, r)| r.split_whitespace().next())
                    .unwrap_or("");
                if !change.is_empty() {
                    let change_dir = project_root.join("openspec/changes").join(change);
                    match crate::spec_framework::spec_slicer::SpecSlicer::build_openspec(
                        &change_dir,
                    ) {
                        Some(vctx) => {
                            let block = vctx.to_prompt_block();
                            info!(
                                "[spec] SpecSlicer: injected virtual context ({} t, task: {:?})",
                                super::token_count::count(&block),
                                vctx.active_task.title
                            );
                            format!("{user_text}\n\n{block}")
                        },
                        None => user_text,
                    }
                } else {
                    user_text
                }
            } else {
                user_text
            };

        // ── Resolve token + model before computing the context budget ────────
        // Fetching models first ensures context_window_size() returns the real
        // limit rather than the 128k fallback, so history truncation is correct
        // even on the very first message of a session (Fix for ADR 0087 §2).
        let api_token = self.ensure_token().await?;
        if self.available_models.is_empty() {
            let provider = self.provider.clone();
            let config = self.provider_config.clone();
            let copilot_api_base = self.copilot_api_base.clone();
            match fetch_models_for_provider(&provider, &config, &api_token, &copilot_api_base).await
            {
                Ok(models) if !models.is_empty() => {
                    info!("Fetched {} models from {}", models.len(), provider.display_name());
                    self.set_models(models, preferred_model);
                },
                Ok(_) => warn!("[{}] model list was empty", provider.display_name()),
                Err(e) => warn!("Could not fetch {} model list: {e}", provider.display_name()),
            }
        }
        let model_id = self.selected_model_id().to_string();
        self.last_submit_model = model_id.clone();

        // Write a session_start record on the very first submit of a new
        // conversation (session_rounds == 0 before the first Done increments it).
        if self.conversation.session_rounds == 0 {
            append_session_start_record(
                &model_id,
                self.provider.display_name(),
                project_root.to_str().unwrap_or(""),
            );
        }

        // ── Build provider settings for this invocation ──────────────────────
        let provider_settings = self.provider.build_settings(
            &self.provider_config,
            api_token.clone(),
            &self.copilot_api_base,
        );
        let effective_token = provider_settings.api_token.clone();
        let chat_endpoint = provider_settings.chat_endpoint.clone();

        let root_display = project_root.display().to_string();

        // ── Codified Context (ADR 0132) ──────────────────────────────────────
        // Reload on first submit or when the project root changes.
        if self.codified_context_enabled {
            let needs_reload = match &self.codified_context {
                None => true,
                Some(cc) => cc.forgiven_dir.parent() != Some(&project_root),
            };
            if needs_reload {
                use crate::config::CodifiedContextConfig;
                let cfg = CodifiedContextConfig {
                    enabled: true,
                    directory: ".forgiven".to_string(),
                    constitution_max_tokens: self.codified_context_constitution_max_tokens,
                    max_specialists_per_turn: self.codified_context_max_specialists,
                    knowledge_fetch_max_bytes: self.codified_context_knowledge_max_bytes,
                };
                self.codified_context =
                    crate::agent::codified_context::CodifiedContext::load(&project_root, &cfg);
            }
        }

        // Tip: show once per session when .forgiven/ is absent (enabled or not).
        if !self.codified_context_tip_shown && self.conversation.session_rounds == 0 {
            let forgiven_present = project_root.join(".forgiven").is_dir();
            if !forgiven_present {
                self.codified_context_tip_shown = true;
                self.conversation.messages.push(ChatMessage {
                    role: Role::System,
                    content: "[tip] Create .forgiven/constitution.md to improve agent \
                              consistency across sessions."
                        .to_string(),
                    images: vec![],
                });
            }
        }

        // ── Cap open-file context injection ─────────────────────────────────
        // The active buffer is useful orientation for small files, but sending
        // tens-of-thousands of tokens on every round (even unrelated rounds)
        // is the primary driver of context bloat (ADR 0087).
        // Cap to MAX_CTX_LINES; the model can call read_file for the rest.
        //
        // Phase 3.1: suppress injection entirely for openspec sessions — the
        // model works from tasks.md/design.md, not the active buffer.  Chat-mode
        // rounds keep the snippet for passive orientation.
        const MAX_CTX_LINES: usize = 150;
        let ctx_total_lines = context.as_ref().map(|c| c.lines().count()).unwrap_or(0);
        let suppress_ctx =
            spec_cmd_ctx.as_ref().map(|(cmd, _)| cmd.starts_with("openspec.")).unwrap_or(false);
        let context_snippet: Option<String> = if suppress_ctx {
            None
        } else {
            context.as_ref().map(|raw| {
                if ctx_total_lines > MAX_CTX_LINES {
                    raw.lines().take(MAX_CTX_LINES).collect::<Vec<_>>().join("\n")
                } else {
                    raw.clone()
                }
            })
        };

        // Build a structural map of src/ symbols (Phase 2.1).  On round 1 this
        // replaces the old filename tree and gives the model symbol-level
        // orientation without extra read_file discovery round-trips.
        // Cached for 30 s alongside cached_project_tree.
        const TREE_TTL: std::time::Duration = std::time::Duration::from_secs(30);
        // Keep project_tree around for the round-2+ stub hint (no filesystem cost).
        let _project_tree = match self.cached_project_tree.as_ref() {
            Some((tree, ts)) if ts.elapsed() < TREE_TTL => tree.clone(),
            _ => {
                let tree = build_project_tree(&project_root, 2);
                self.cached_project_tree = Some((tree.clone(), std::time::Instant::now()));
                tree
            },
        };
        let structural_map = match self.cached_structural_map.as_ref() {
            Some((map, ts)) if ts.elapsed() < TREE_TTL => map.clone(),
            _ => {
                let map = build_structural_map(&project_root);
                self.cached_structural_map = Some((map.clone(), std::time::Instant::now()));
                map
            },
        };

        let use_planning = provider_settings.planning_tools;

        let planning_conventions = if use_planning {
            "- Tasks (≥3 distinct file ops): create_task per step before work; complete_task after.\n\
- ask_user: only for ambiguous destructive actions or mutually exclusive design choices.\n"
        } else {
            ""
        };

        let planning_tool_entries = if use_planning {
            "- create_task, complete_task — plan/track multi-step jobs\n\
- ask_user, ask_user_input — ask user a question or collect free-text input\n"
        } else {
            ""
        };

        let tool_rules = format!(
            "CONVENTIONS:\n\
- Symbol tools first: get_file_outline → get_symbol_context before read_file.\n\
  Use read_file only when you need more than 3 symbols from the same file.\n\
- Edits: edit_file over write_file; copy old_str verbatim; retry with fresh read on mismatch.\n\
- Batch: read_files([…]) over repeated read_file; search_files over read_file+scan.\n\
- Work silently; write one concise summary after all tools finish.\n\
{planning_conventions}\
- Memory (when tools available): search_nodes(\"project context\") on first call;\n\
  add_observations for non-obvious discoveries; persist key facts at session end.\n\
\n\
Tools:\n\
- get_file_outline, get_symbol_context — symbol-level retrieval (prefer these)\n\
- read_file — full file (expensive; use when >3 symbols needed)\n\
- read_files, search_files — batch reads and pattern search\n\
- write_file, edit_file — create/overwrite or surgical find-and-replace\n\
- list_directory — list directory contents\n\
- expand_result(id) — retrieve full content of a truncated tool result\n\
{planning_tool_entries}"
        );

        // Only include the full project tree on round 1 (session_rounds == 0).
        // Round 1: inject structural map (symbol names per file) instead of the
        // old filename-only tree.  Gives the model symbol-level orientation
        // (≈200–400 t) so it can skip discovery read_file calls.
        // Subsequent rounds: one-line stub saves 300–500 tokens per round.
        let tree_block = if self.conversation.session_rounds == 0 {
            format!("{structural_map}\n\n")
        } else {
            String::from("[Project tree omitted after round 1 — call list_directory if needed]\n\n")
        };

        // Codified context block (constitution + triggered specialists + knowledge catalogue).
        // Empty string when disabled or .forgiven/ absent.
        // Derive open file path from context header ("File: path/to/file\n\n...").
        let open_file_path = context
            .as_ref()
            .and_then(|s| s.lines().next())
            .and_then(|line| line.strip_prefix("File: "))
            .unwrap_or("")
            .to_string();
        let codified_block = if self.codified_context_enabled {
            self.codified_context
                .as_ref()
                .map(|cc| cc.system_prompt_block(&open_file_path, &user_text))
                .unwrap_or_default()
        } else {
            String::new()
        };

        let system = if let Some(ref ctx) = context_snippet {
            let truncation_note = if ctx_total_lines > MAX_CTX_LINES {
                format!(
                    "\n[Showing first {MAX_CTX_LINES} of {ctx_total_lines} lines — \
                     call read_file for the full content]"
                )
            } else {
                String::new()
            };
            format!(
                "You are an agentic coding assistant embedded in the 'forgiven' terminal editor.\n\
                 Project root: {root_display}\n\n\
                 {tree_block}\
                 {codified_block}\
                 {tool_rules}\n\
                 Currently open file (use read_file for full content):\n\
                 ```\n{ctx}{truncation_note}\n```"
            )
        } else {
            format!(
                "You are an agentic coding assistant embedded in the 'forgiven' terminal editor.\n\
                 Project root: {root_display}\n\n\
                 {tree_block}\
                 {codified_block}\
                 {tool_rules}"
            )
        };

        // For the Anthropic provider, split the system prompt into a stable
        // prefix (preamble + structural map + tool_rules) and the volatile
        // context_snippet so that the stable prefix is eligible for prompt
        // caching.  Other providers (Copilot, OpenAI) cache automatically on
        // the prefix — plain string content is correct for them.
        let system_message = if self.provider == ProviderKind::Anthropic {
            if let Some(ref _ctx) = context_snippet {
                // Find where context_snippet starts in the assembled system
                // string and split there.
                let ctx_marker = "Currently open file";
                if let Some(split_pos) = system.find(ctx_marker) {
                    let stable = &system[..split_pos];
                    let volatile = &system[split_pos..];
                    serde_json::json!({
                        "role": "system",
                        "content": [
                            {
                                "type": "text",
                                "text": stable,
                                "cache_control": { "type": "ephemeral" }
                            },
                            { "type": "text", "text": volatile }
                        ]
                    })
                } else {
                    // Couldn't find split point — cache the whole thing.
                    serde_json::json!({
                        "role": "system",
                        "content": [
                            {
                                "type": "text",
                                "text": system,
                                "cache_control": { "type": "ephemeral" }
                            }
                        ]
                    })
                }
            } else {
                // No context_snippet — entire system prompt is stable.
                serde_json::json!({
                    "role": "system",
                    "content": [
                        {
                            "type": "text",
                            "text": system,
                            "cache_control": { "type": "ephemeral" }
                        }
                    ]
                })
            }
        } else {
            serde_json::json!({ "role": "system", "content": system })
        };
        let mut send_messages: Vec<serde_json::Value> = vec![system_message];

        // ── Token-aware history truncation with importance scoring ───────────
        // Estimate tokens using the chars/4 approximation (1 token ≈ 4 chars).
        // Budget is 80% of the model's context window minus an estimate for the
        // system prompt, so we never approach the hard API limit.
        // context_limit already resolved above (before file-block assembly).
        let system_tokens = (system.len() / 4) as u32;
        let budget = (context_limit * 4 / 5).saturating_sub(system_tokens);

        // Snapshot for per-invocation metrics written on Done.
        self.last_submit_ctx = Some(SubmitCtx {
            ctx_window: context_limit,
            sys_tokens: system_tokens,
            budget_for_history: budget,
        });

        // ── Context budget audit log ─────────────────────────────────────────
        // Visible in SPC d → Recent Logs. One line per submission so you can
        // track how much of the window each component consumes.
        let ctx_file_tokens = context_snippet.as_ref().map(|c| c.len() / 4).unwrap_or(0);
        info!(
            "[ctx] window={}t  sys={}t (rules≈{}t + file≈{}t{})  history_msgs={}  \
             budget_for_history={}t",
            context_limit,
            system_tokens,
            (system.len() - context_snippet.as_ref().map(|c| c.len()).unwrap_or(0)) / 4,
            ctx_file_tokens,
            if ctx_total_lines > MAX_CTX_LINES {
                format!(" [{}/{}lines]", MAX_CTX_LINES, ctx_total_lines)
            } else {
                String::new()
            },
            self.conversation.messages.iter().filter(|m| !matches!(m.role, Role::System)).count(),
            budget,
        );
        if system_tokens > context_limit / 2 {
            warn!(
                "[ctx] System prompt alone ({system_tokens}t) exceeds 50% of context window \
                 ({context_limit}t) — the open file ({ctx_file_tokens}t) is the likely cause. \
                 Close the file or switch to a model with a larger context window."
            );
        }

        // ── Phase 1: always keep the most recent MIN_RECENT non-system messages.
        const MIN_RECENT: usize = 4;
        let non_system: Vec<usize> = self
            .conversation
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| !matches!(m.role, Role::System))
            .map(|(i, _)| i)
            .collect();
        let recent_start_idx = non_system.len().saturating_sub(MIN_RECENT);
        let recent_indices: std::collections::HashSet<usize> =
            non_system[recent_start_idx..].iter().copied().collect();

        // Token cost of the guaranteed-recent slice.
        let recent_tokens: u32 = recent_indices
            .iter()
            .map(|&i| (self.conversation.messages[i].content.len() / 4) as u32 + 4)
            .sum();
        let older_budget = budget.saturating_sub(recent_tokens);

        // ── Phase 2: from older messages, greedily include highest-importance
        //    ones first until the older_budget is exhausted, then reassemble in
        //    original order so conversation coherence is preserved.
        let mut candidates: Vec<(usize, u32, u32)> = self
            .conversation
            .messages
            .iter()
            .enumerate()
            .filter(|(i, m)| !matches!(m.role, Role::System) && !recent_indices.contains(i))
            .map(|(i, m)| {
                let tokens = (m.content.len() / 4) as u32 + 4;
                let score = super::message_importance(m);
                (i, tokens, score)
            })
            .collect();

        // Sort by score descending so high-value messages are included first.
        candidates.sort_by(|a, b| b.2.cmp(&a.2).then(b.0.cmp(&a.0)));

        let mut used: u32 = 0;
        let mut included: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for (i, tokens, _) in &candidates {
            if used + tokens > older_budget {
                continue; // skip this one — too large — try smaller/higher-scored ones
            }
            used += tokens;
            included.insert(*i);
        }

        // Emit messages in original order (older included + recent).
        // Apply observation masking: non-recent assistant messages longer than
        // the threshold are replaced with a stub to reduce re-send token cost.
        // The display history (self.conversation.messages) is never modified.
        for (i, msg) in self.conversation.messages.iter().enumerate() {
            if matches!(msg.role, Role::System) {
                continue;
            }
            if !included.contains(&i) && !recent_indices.contains(&i) {
                continue;
            }
            let content = if observation_mask_threshold_chars > 0
                && !recent_indices.contains(&i)
                && matches!(msg.role, Role::Assistant)
                && msg.content.len() > observation_mask_threshold_chars
            {
                let tok_est = msg.content.len() / 4;
                format!(
                    "[assistant output: ~{tok_est} tokens — \
                     truncated for re-send; call the relevant tool again if needed]"
                )
            } else {
                msg.content.clone()
            };
            send_messages.push(serde_json::json!({
                "role": msg.role.as_str(),
                "content": content
            }));
        }
        // When images are attached, use the OpenAI content-array format so the
        // model receives both text and vision inputs.  Otherwise use a plain string.
        let image_dims: Vec<(u32, u32)> =
            images.iter().map(|img| (img.width, img.height)).collect();

        // ── Intent translation (SPC a t / [agent.intent_translator]) ─────────
        // Translate the raw message into a structured task spec before dispatch.
        // Falls through silently (uses raw message) on timeout, error, or short message.
        let (api_user_text, intent_preamble): (String, Option<String>) =
            if self.intent_translator_enabled {
                let open_file_hint = context
                    .as_ref()
                    .and_then(|c| c.strip_prefix("File: "))
                    .and_then(|c| c.lines().next());
                let language_hint = open_file_hint
                    .and_then(|p| std::path::Path::new(p).extension())
                    .and_then(|e| e.to_str())
                    .map(|ext| match ext {
                        "rs" => "Rust",
                        "ts" | "tsx" => "TypeScript",
                        "py" => "Python",
                        "go" => "Go",
                        "js" | "jsx" => "JavaScript",
                        "cpp" | "cc" | "cxx" => "C++",
                        "c" | "h" => "C",
                        "java" => "Java",
                        "rb" => "Ruby",
                        "swift" => "Swift",
                        _ => "unknown",
                    });
                let tx_ctx = super::intent::TranslationContext {
                    open_file: open_file_hint,
                    recent_files: &[],
                    project_root: &project_root,
                    language_hint,
                };
                // Resolve endpoint/token/model for the translator's chosen provider.
                // "ollama" → local Ollama, no auth; "active" → reuse main provider.
                let (tx_endpoint, tx_token, tx_model, tx_kind) =
                    if self.intent_translator_provider == "ollama" {
                        let ep =
                            format!("{}/v1/chat/completions", self.provider_config.ollama_base_url);
                        (
                            ep,
                            String::new(),
                            self.intent_translator_ollama_model.clone(),
                            ProviderKind::Ollama,
                        )
                    } else {
                        (
                            chat_endpoint.clone(),
                            effective_token.clone(),
                            self.intent_translator_model.clone(),
                            self.provider.clone(),
                        )
                    };
                let tx_settings = super::intent::IntentCallSettings {
                    endpoint: &tx_endpoint,
                    api_token: &tx_token,
                    model: &tx_model,
                    provider_kind: &tx_kind,
                    timeout_ms: self.intent_translator_timeout_ms,
                    min_chars_to_translate: self.intent_translator_min_chars,
                    skip_patterns: &self.intent_translator_skip_patterns,
                    openrouter_site_url: &self.provider_config.openrouter_site_url,
                    openrouter_app_name: &self.provider_config.openrouter_app_name,
                };
                match super::intent::translate_intent(&user_text, &tx_ctx, &tx_settings).await {
                    Some(intent) if !intent.ambiguities.is_empty() => {
                        // Ambiguous: show clarifying questions and bail out without
                        // dispatching the agent loop. The user refines and resubmits.
                        let questions = intent.ambiguities.join("\n• ");
                        self.conversation.messages.push(ChatMessage {
                            role: Role::System,
                            content: format!(
                                "Intent unclear — please clarify before submitting:\n• {questions}"
                            ),
                            images: vec![],
                        });
                        self.conversation.messages.push(ChatMessage {
                            role: Role::User,
                            content: user_text,
                            images: image_dims,
                        });
                        self.scroll = 0;
                        self.status = AgentStatus::Idle;
                        return Ok(());
                    },
                    Some(intent) if !intent.structured_prompt.is_empty() => {
                        let preamble = super::intent::format_preamble(&intent);
                        (intent.structured_prompt, Some(preamble))
                    },
                    _ => (user_text.clone(), None),
                }
            } else {
                (user_text.clone(), None)
            };

        let user_msg = if images.is_empty() {
            serde_json::json!({ "role": "user", "content": api_user_text.clone() })
        } else {
            let mut content_parts: Vec<serde_json::Value> = Vec::new();
            if !api_user_text.trim().is_empty() {
                content_parts.push(serde_json::json!({
                    "type": "text",
                    "text": api_user_text.clone()
                }));
            }
            for img in &images {
                content_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": { "url": img.data_uri, "detail": "auto" }
                }));
            }
            serde_json::json!({ "role": "user", "content": content_parts })
        };
        send_messages.push(user_msg);

        // ── Context breakdown for diagnostics / fuel gauge (Phase 1) ─────────
        // Compute per-segment token counts now that send_messages is fully assembled.
        // history = send_messages[1..n-1] (everything except system[0] and user[-1]).
        //
        // Uses the same len/4 approximation as history truncation (above) rather
        // than calling tiktoken here.  This keeps the breakdown numbers consistent
        // with the budget the truncation algorithm actually used, and avoids
        // O(N × tokeniser) cost on every submit for display-only numbers.
        let history_t: u32 = send_messages[1..send_messages.len().saturating_sub(1)]
            .iter()
            .map(|v| (v["content"].as_str().unwrap_or("").len() / 4) as u32)
            .sum();
        let ctx_file_t = context_snippet.as_ref().map(|c| (c.len() / 4) as u32).unwrap_or(0);
        // system_t is already computed via len/4 above (system_tokens); reuse it.
        let system_t = system_tokens;
        let user_msg_t = (user_text.len() / 4) as u32;
        self.last_breakdown = Some(ContextBreakdown {
            sys_rules_t: system_t.saturating_sub(ctx_file_t),
            ctx_file_t,
            history_t,
            user_msg_t,
            ctx_window: context_limit,
        });

        // Show the intent preamble (dim System line) above the user message.
        if let Some(preamble) = intent_preamble {
            self.conversation.messages.push(ChatMessage {
                role: Role::System,
                content: preamble,
                images: vec![],
            });
        }
        self.conversation.messages.push(ChatMessage {
            role: Role::User,
            content: user_text,
            images: image_dims,
        });

        self.scroll = 0;
        // Pre-allocate enough capacity for a typical streaming response to avoid
        // repeated heap reallocations as tokens accumulate via push_str.
        self.streaming_reply = Some(String::with_capacity(4096));
        self.tasks.clear();

        self.status = if self.janitor_compressing {
            AgentStatus::Compressing
        } else {
            AgentStatus::WaitingForResponse { round: 1 }
        };

        let (tx, rx) = mpsc::channel::<StreamEvent>(128);
        self.stream_rx = Some(rx);
        self.usage_received_this_round = false;

        let (cont_tx, cont_rx) = mpsc::unbounded_channel::<bool>();
        self.continuation_tx = Some(cont_tx);

        let (question_tx, question_rx) = mpsc::unbounded_channel::<String>();
        self.question_tx = Some(question_tx);

        let (abort_tx, abort_rx) = oneshot::channel::<()>();
        self.abort_tx = Some(abort_tx);

        let mcp = self.mcp_manager.as_ref().map(Arc::clone);
        let knowledge_docs: Vec<(String, PathBuf)> = self
            .codified_context
            .as_ref()
            .map(|cc| cc.knowledge_docs.iter().map(|d| (d.name.clone(), d.path.clone())).collect())
            .unwrap_or_default();
        let knowledge_fetch_max_bytes = self.codified_context_knowledge_max_bytes;
        tokio::spawn(agentic_loop(
            provider_settings,
            send_messages,
            project_root,
            tx,
            model_id,
            max_rounds,
            warning_threshold,
            cont_rx,
            question_rx,
            abort_rx,
            mcp,
            auto_compress,
            expand_threshold_chars,
            knowledge_docs,
            knowledge_fetch_max_bytes,
        ));
        Ok(())
    }

    /// Launch a single-round, no-tool LLM request for the inline assistant (ADR 0111).
    ///
    /// Builds a minimal 2-message conversation (system + user), spawns `agentic_loop`
    /// with `max_rounds = 1` and tool calling disabled, and returns the streaming channel
    /// pair.  The caller (Editor) wraps these in an `InlineAssistState`.
    pub async fn start_inline_assist(
        &mut self,
        selection_text: String,
        prompt: String,
        project_root: PathBuf,
        language: Option<String>,
    ) -> Result<(mpsc::Receiver<StreamEvent>, oneshot::Sender<()>)> {
        let api_token = self.ensure_token().await?;
        let model_id = self.selected_model_id().to_string();

        // Disable tool calling — inline assist is pure text generation.
        let mut provider_settings =
            self.provider.build_settings(&self.provider_config, api_token, &self.copilot_api_base);
        provider_settings.supports_tool_calls = false;
        provider_settings.planning_tools = false;

        let lang_str = language.as_deref().unwrap_or("code");
        let system_prompt = format!(
            "You are a {lang_str} code transformation engine. \
            You receive a CODE block and a DIRECTIVE. \
            You output ONLY the transformed {lang_str} code — \
            no conversation, no explanation, no markdown fences, no preamble. \
            If the code is empty, output only what was asked for. \
            Preserve the original indentation."
        );

        let user_content = if selection_text.is_empty() {
            format!("CODE:\n(none)\n\nDIRECTIVE: {prompt}")
        } else {
            format!("CODE:\n{selection_text}\n\nDIRECTIVE: {prompt}")
        };

        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt }),
            serde_json::json!({ "role": "user", "content": user_content }),
        ];

        let (tx, rx) = mpsc::channel::<StreamEvent>(128);
        let (abort_tx, abort_rx) = oneshot::channel::<()>();
        // Dummy continuation + question channels — inline assist never uses them.
        let (_cont_tx, cont_rx) = mpsc::unbounded_channel::<bool>();
        let (_question_tx, question_rx) = mpsc::unbounded_channel::<String>();

        tokio::spawn(agentic_loop(
            provider_settings,
            messages,
            project_root,
            tx,
            model_id,
            1, // max_rounds
            0, // warning_threshold
            cont_rx,
            question_rx,
            abort_rx,
            None,  // no MCP manager
            false, // auto_compress
            0,     // expand_threshold — disabled for inline assist
            vec![],
            0,
        ));

        Ok((rx, abort_tx))
    }

    /// Spin up a single-round, tool-enabled investigation subagent (Phase 3.3).
    ///
    /// The query comes from `self.conversation.input`.  The agentic loop runs for at most one
    /// round with all tools available so the model can explore the codebase.
    /// Tokens are collected in `self.investigation_buf`; when `Done` arrives in
    /// `poll_stream()` the summary is injected as a System message into the main
    /// session and `investigation_rx` is cleared.
    ///
    /// Callers must ensure `self.conversation.input` is non-empty before calling.
    pub async fn start_investigation_agent(
        &mut self,
        project_root: PathBuf,
        preferred_model: &str,
    ) -> Result<()> {
        if self.conversation.input.trim().is_empty() {
            return Ok(());
        }

        let api_token = self.ensure_token().await?;
        let model_id = self.selected_model_id_with_fallback(preferred_model).to_string();

        let mut provider_settings =
            self.provider.build_settings(&self.provider_config, api_token, &self.copilot_api_base);
        provider_settings.planning_tools = false; // investigation doesn't create tasks

        let root_display = project_root.display().to_string();
        let system_prompt = format!(
            "You are a code investigator for the 'forgiven' terminal editor.\n\
             Project root: {root_display}\n\n\
             INVESTIGATION RULES:\n\
             - Explore the codebase using get_file_outline, get_symbol_context, search_files, \
               and read_file as needed.\n\
             - Make NO edits — this is read-only exploration.\n\
             - After exploring, output a COMPACT SUMMARY (max 200 words) covering:\n\
               * Which files/functions are involved\n\
               * Key call paths or data flow\n\
               * Any non-obvious facts the developer should know\n\
             - No preamble, no pleasantries. Start directly with the findings."
        );

        let query = std::mem::take(&mut self.conversation.input);
        self.conversation.input_cursor = 0;

        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt }),
            serde_json::json!({ "role": "user", "content": query }),
        ];

        let (tx, rx) = mpsc::channel::<StreamEvent>(128);
        let (_abort_tx, abort_rx) = oneshot::channel::<()>();
        let (_cont_tx, cont_rx) = mpsc::unbounded_channel::<bool>();
        let (_question_tx, question_rx) = mpsc::unbounded_channel::<String>();

        let mcp = self.mcp_manager.clone();
        tokio::spawn(agentic_loop(
            provider_settings,
            messages,
            project_root,
            tx,
            model_id,
            1, // max_rounds — single exploration pass
            0, // warning_threshold
            cont_rx,
            question_rx,
            abort_rx,
            mcp,
            false, // auto_compress
            0,     // expand_threshold — disabled for investigation subagent
            vec![],
            0,
        ));

        self.investigation_rx = Some(rx);
        self.investigation_buf.clear();
        self.status = AgentStatus::Investigating;
        Ok(())
    }
}
