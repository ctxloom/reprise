//! reprise MCP server (docs/SERVERS.md §5) — a stdio Model Context Protocol
//! server exposing reprise clone-detection to coding agents. Tools are thin
//! wrappers over the runtime-free core in `lib.rs`; this file is only the
//! `rmcp` transport + tool routing. Report-only: reprise advises, the agent acts.
//!
//! The `reprise` scan/check are synchronous and CPU-bound (rayon-parallel), so
//! each tool runs them on `spawn_blocking` — a full scan must never stall the
//! async reactor that is also driving stdin/stdout.

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ScanRequest {
    /// Filesystem path to the repository or directory to scan.
    path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct CheckRequest {
    /// Filesystem path to the repository to check.
    path: String,
    /// Base git ref to diff against (commit/tag/branch). Omit to use the
    /// configured `[baseline] ref`, else the merge-base with the default branch.
    #[serde(default)]
    base: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct FindSimilarRequest {
    /// Filesystem path to the repository to search.
    path: String,
    /// The candidate code — a complete function / method definition.
    snippet: String,
    /// Language of the snippet: rust | python | typescript | tsx | go | kotlin | c
    /// (or the matching file extension, e.g. `rs`).
    lang: String,
}

#[derive(Clone)]
struct Reprise {
    // Consumed at runtime by the `#[tool_handler]`-generated dispatch (verified:
    // the stdio smoke test lists and calls both tools). rustc's dead-code pass
    // can't see through the macro expansion, so the read looks absent here.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Reprise {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Scan a repository for duplicate / near-duplicate code (clones) with reprise. \
        Returns the full findings report as JSON: clone groups with per-member file:line locations, \
        tier, similarity, and an anti-unification template marking where the copies diverge. \
        Use to audit a codebase, or before writing a new helper to check it does not already exist."
    )]
    async fn reprise_scan(
        &self,
        Parameters(ScanRequest { path }): Parameters<ScanRequest>,
    ) -> Result<CallToolResult, McpError> {
        let out = tokio::task::spawn_blocking(move || reprise_mcp::scan_json(&path))
            .await
            .map_err(|e| {
                McpError::internal_error(format!("scan task failed to join: {e}"), None)
            })?;
        into_result(out, "reprise scan failed")
    }

    #[tool(
        description = "Check a repository for duplication DRIFT versus a base git ref (default: the \
        merge-base with the default branch). Returns JSON including `inconsistent-update` findings — \
        a change edited some but not all members of a known duplicate group, with the untouched copies \
        named and the divergence trend. Use to gate a change or PR against clone drift."
    )]
    async fn reprise_check(
        &self,
        Parameters(CheckRequest { path, base }): Parameters<CheckRequest>,
    ) -> Result<CallToolResult, McpError> {
        let out =
            tokio::task::spawn_blocking(move || reprise_mcp::check_json(&path, base.as_deref()))
                .await
                .map_err(|e| {
                    McpError::internal_error(format!("check task failed to join: {e}"), None)
                })?;
        into_result(out, "reprise check failed")
    }

    #[tool(
        description = "Before writing a helper, check whether the repository already contains an \
        equivalent one. Normalizes the given SNIPPET (a complete function/method) and matches it \
        against every same-language unit in the repo: reports existing units it duplicates — exact \
        (identical after rename/literal/loop-form normalization) or near-miss under the clone \
        threshold — with their file:line, similarity, and a template showing the shared skeleton. \
        Use to avoid reimplementing an existing helper; call the reported unit instead."
    )]
    async fn reprise_find_similar(
        &self,
        Parameters(FindSimilarRequest {
            path,
            snippet,
            lang,
        }): Parameters<FindSimilarRequest>,
    ) -> Result<CallToolResult, McpError> {
        let out = tokio::task::spawn_blocking(move || {
            reprise_mcp::find_similar_json(&path, &snippet, &lang)
        })
        .await
        .map_err(|e| {
            McpError::internal_error(format!("find_similar task failed to join: {e}"), None)
        })?;
        into_result(out, "reprise find_similar failed")
    }
}

/// Project a core `anyhow::Result<String>` (JSON) onto an MCP tool result: JSON
/// text content on success; on failure a tool-level error result (`isError:
/// true`) carrying the reprise failure text, so the model sees it and can
/// self-correct. A tool-execution failure — a nonexistent path, a malformed
/// `reprise.toml`, an unsupported-language rejection — is *not* a JSON-RPC
/// protocol fault, so it must not become an `McpError` (which MCP clients
/// render opaquely, hiding the message). `McpError` stays reserved for genuine
/// transport/parameter faults (e.g. a task that fails to join).
fn into_result(out: anyhow::Result<String>, context: &str) -> Result<CallToolResult, McpError> {
    match out {
        Ok(json) => Ok(CallToolResult::success(vec![ContentBlock::text(json)])),
        Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
            "{context}: {e:#}"
        ))])),
    }
}

#[tool_handler]
impl ServerHandler for Reprise {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "reprise finds duplicate and near-duplicate code within a repository (same-language \
                 only) and gates duplication drift. Report-only: it never modifies code. Call \
                 `reprise_scan` to audit a repo, `reprise_check` to gate a change against a base ref."
                    .to_string(),
            )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = Reprise::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(result: &CallToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|c| c.as_text())
            .map(|t| t.text.clone())
            .collect()
    }

    #[test]
    fn tool_failure_is_a_tool_level_error_not_a_protocol_error() {
        // A failure that originates *inside* the tool (here: a simulated bad
        // input) must surface as a `CallToolResult` with `isError: true`, so
        // the model sees the text and can self-correct — never as an
        // `McpError` protocol fault (which clients render opaquely).
        let out = into_result(
            Err(anyhow::anyhow!("no such path: /nope")),
            "reprise scan failed",
        );
        let result = out.expect("tool failures must not become protocol errors");
        assert_eq!(result.is_error, Some(true));
        assert!(
            text_of(&result).contains("reprise scan failed: no such path: /nope"),
            "error content must carry the context + reprise failure text"
        );
    }

    #[test]
    fn tool_success_carries_json_content() {
        let result = into_result(Ok("{\"groups\":[]}".to_string()), "reprise scan failed")
            .expect("success path");
        assert_eq!(result.is_error, Some(false));
        assert_eq!(text_of(&result), "{\"groups\":[]}");
    }
}
