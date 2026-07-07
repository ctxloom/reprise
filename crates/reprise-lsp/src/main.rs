//! reprise LSP server (docs/SERVERS.md §4) — surfaces reprise clone findings as
//! advisory (Information) diagnostics. A clone group is projected to one
//! diagnostic per member, with the sibling members as `relatedInformation`
//! (§4.2) and the structural fingerprint in `data` as stable identity across
//! drift. Analyzes on save (D-SRV-4): a whole-repo scan runs on `spawn_blocking`
//! and diagnostics are (re)published for every file with findings. Report-only —
//! it publishes advice, never edits code.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use reprise_server_core::diagnostics::{self, Finding};
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

struct Backend {
    client: Client,
    /// Workspace root captured at `initialize`.
    root: Mutex<Option<PathBuf>>,
    /// Files we last published diagnostics to, so stale ones can be cleared when
    /// a rescan no longer finds them.
    published: Mutex<HashSet<PathBuf>>,
    /// Serializes `refresh`. tower-lsp-server dispatches notification handlers
    /// concurrently (the `LanguageServer` bound is `Send + Sync`), so an
    /// editor "save all" can overlap several `refresh` calls. Without this,
    /// they would run N redundant full-workspace scans and race the
    /// `published` read-modify-write into a lost update — leaving a stale
    /// diagnostic that no later rescan would ever clear. An *async* mutex
    /// because it is held across the scan's `.await` points; a `std` mutex
    /// must never be held across an await.
    refresh_lock: tokio::sync::Mutex<()>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            root: Mutex::new(None),
            published: Mutex::new(HashSet::new()),
            refresh_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Rescan the workspace and (re)publish diagnostics, clearing files that no
    /// longer have findings. No-op until a root is known. Serialized end-to-end
    /// via `refresh_lock` so concurrent invocations neither duplicate scans nor
    /// race the `published` update (see the field doc). The `std::sync::Mutex`
    /// guards (`root`, `published`) are each released before any `.await`, so
    /// the returned future stays `Send`; only the async `refresh_lock` guard is
    /// held across awaits.
    async fn refresh(&self) {
        // Hold this for the whole scan→publish→published-update so it is atomic
        // w.r.t. concurrent `refresh` calls and redundant overlapping scans
        // collapse (the later caller waits, then re-scans once).
        let _refresh_guard = self.refresh_lock.lock().await;

        let Some(root) = self.root.lock().unwrap().clone() else {
            return;
        };

        let scanned = tokio::task::spawn_blocking(move || {
            let cfg = reprise::Config::load(&root)?;
            let report = reprise::scan(&root, &cfg)?;
            anyhow::Ok(report)
        })
        .await;

        let report = match scanned {
            Ok(Ok(report)) => report,
            Ok(Err(e)) => {
                self.log_error(format!("reprise scan failed: {e:#}")).await;
                return;
            }
            Err(e) => {
                self.log_error(format!("scan task panicked: {e}")).await;
                return;
            }
        };

        let files = diagnostics::project(&report.groups);
        let mut current: HashSet<PathBuf> = HashSet::new();
        for ff in &files {
            let Some(uri) = Uri::from_file_path(&ff.file) else {
                continue;
            };
            let diags: Vec<Diagnostic> = ff.findings.iter().map(to_diagnostic).collect();
            self.client.publish_diagnostics(uri, diags, None).await;
            current.insert(ff.file.clone());
        }

        // Clear files that had findings before but not now.
        let stale: Vec<PathBuf> = {
            let published = self.published.lock().unwrap();
            published.difference(&current).cloned().collect()
        };
        for old in stale {
            if let Some(uri) = Uri::from_file_path(&old) {
                self.client.publish_diagnostics(uri, Vec::new(), None).await;
            }
        }
        *self.published.lock().unwrap() = current;
    }

    async fn log_error(&self, msg: String) {
        self.client.log_message(MessageType::ERROR, msg).await;
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // `root_uri` is deprecated in favour of `workspace_folders`, but remains
        // the fallback for clients that only send the former.
        #[allow(deprecated)]
        let root = params
            .workspace_folders
            .as_ref()
            .and_then(|ws| ws.first())
            .map(|f| f.uri.clone())
            .or(params.root_uri.clone())
            .and_then(|uri| uri.to_file_path().map(|p| p.into_owned()));
        if let Some(r) = root {
            *self.root.lock().unwrap() = Some(r);
        }
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "reprise-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "reprise-lsp ready; scanning workspace")
            .await;
        self.refresh().await;
    }

    async fn did_save(&self, _: DidSaveTextDocumentParams) {
        self.refresh().await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// One reprise line span → an LSP range covering those whole lines. reprise line
/// spans are 1-based inclusive; LSP positions are 0-based and the range end is
/// exclusive, so the 1-based `end` line is used directly as the exclusive 0-based
/// line after the span (a single-line unit still spans its whole line).
fn range_of((start, end): (u32, u32)) -> Range {
    Range::new(
        Position::new(start.saturating_sub(1), 0),
        Position::new(end, 0),
    )
}

/// Map a neutral `Finding` onto an advisory LSP diagnostic: Information severity,
/// tier as `code`, siblings as `relatedInformation`, and the stable identity
/// (group id + structural fingerprint) in `data` for later code actions.
fn to_diagnostic(f: &Finding) -> Diagnostic {
    let related = f
        .related
        .iter()
        .filter_map(|r| {
            Uri::from_file_path(&r.file).map(|uri| DiagnosticRelatedInformation {
                location: Location::new(uri, range_of(r.line_span)),
                message: format!("clone member: {}", r.name),
            })
        })
        .collect();
    Diagnostic {
        range: range_of(f.line_span),
        severity: Some(DiagnosticSeverity::INFORMATION),
        code: Some(NumberOrString::String(f.tier.clone())),
        source: Some("reprise".into()),
        message: f.message.clone(),
        related_information: Some(related),
        data: Some(serde_json::json!({
            "groupId": f.group_id,
            "fingerprint": f.fingerprint,
        })),
        ..Default::default()
    }
}

#[tokio::main]
async fn main() {
    let (service, socket) = LspService::new(Backend::new);
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use reprise_server_core::diagnostics::Related;

    #[test]
    fn range_maps_1based_inclusive_to_0based_exclusive() {
        let r = range_of((10, 20));
        assert_eq!(r.start, Position::new(9, 0));
        assert_eq!(r.end, Position::new(20, 0));
        // A single-line unit still spans its whole line.
        let one = range_of((5, 5));
        assert_eq!(one.start, Position::new(4, 0));
        assert_eq!(one.end, Position::new(5, 0));
    }

    #[test]
    fn finding_maps_to_advisory_diagnostic() {
        // Uri::from_file_path canonicalizes and requires the file to exist, so
        // point the related member at a real file (this test binary).
        let real = std::env::current_exe().unwrap();
        let f = Finding {
            line_span: (1, 3),
            tier: "near-normalized".into(),
            similarity: 0.98,
            group_id: "g".into(),
            fingerprint: "fp".into(),
            message: "msg".into(),
            related: vec![Related {
                file: real,
                line_span: (10, 12),
                name: "sibling".into(),
            }],
        };
        let d = to_diagnostic(&f);
        assert_eq!(d.severity, Some(DiagnosticSeverity::INFORMATION));
        assert_eq!(d.source.as_deref(), Some("reprise"));
        assert_eq!(d.related_information.as_ref().unwrap().len(), 1);
        assert!(matches!(d.code, Some(NumberOrString::String(ref s)) if s == "near-normalized"));
    }
}
