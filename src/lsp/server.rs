//! tower-lsp based Language Server implementation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::{Mutex, Semaphore};
use tower_lsp::jsonrpc::{Error, Result};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use super::hover::build_hover_content;
use crate::analyze::coverage::StepValidation;
use crate::analyze::file::{analyze_file, FileAnalysis, SpecResolver};
use crate::analyze::matcher::MatchResult;
use crate::analyze::scanner::{find_url_at_position, SpecUrl};

use crate::effects::model::{
    EffectSummaryResult, EffectsMode, EffectsOptions, EffectsRequest, EffectsStatus,
    ExplainEffectsRequest, ExplainEffectsResult, SubjectSelector,
};
use crate::effects::render::{
    editor_categories, editor_details_markdown, editor_effect_details, editor_summary_markdown,
    explanation_markdown, EditorCategory, EditorEffectDetails,
};
use crate::model::QueryResult;

const DEBOUNCE_DELAY_MS: u64 = 300;
const DEFAULT_MAX_BADGES: usize = 3;
const EFFECT_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Keep the permit inside the blocking task: cancelling an LSP request must
/// not let another analysis start while its blocking work is still running.
async fn run_effect_work<T: Send + 'static>(
    worker: &Arc<Semaphore>,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::result::Result<T, tokio::task::JoinError> {
    let permit = Arc::clone(worker)
        .acquire_owned()
        .await
        .expect("effect worker is never closed");
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
}

/// Effects configuration supplied by the CLI. Editor initialization options may
/// further disable effects or change the display limit.
#[derive(Debug, Clone)]
pub struct LspOptions {
    pub effects_enabled: bool,
    pub max_badges: usize,
    pub rule_paths: Vec<String>,
    pub environment: String,
}

impl Default for LspOptions {
    fn default() -> Self {
        Self {
            effects_enabled: true,
            max_badges: DEFAULT_MAX_BADGES,
            rule_paths: Vec::new(),
            environment: crate::effects::model::DEFAULT_ENVIRONMENT.into(),
        }
    }
}

/// Versioned cache entry.
#[derive(Clone)]
struct Versioned<T: Clone> {
    version: i32,
    data: T,
}

#[derive(Clone)]
struct CachedEffect {
    summary: EffectSummaryResult,
    markdown: String,
}

enum EffectLookup {
    Cached(Arc<CachedEffect>),
    Claimed(String),
    Pending,
}

struct EffectJob {
    subject: SubjectSelector,
    expected_sha: String,
    document: (String, i32),
    options: EffectsOptions,
    fingerprint: String,
    key: String,
}

/// Shared state that can be cloned into spawned tasks via Arc.
struct State {
    client: Client,
    fuzzy_threshold: Mutex<f64>,
    spec_urls: Mutex<Option<Vec<SpecUrl>>>,
    query_cache: DashMap<String, QueryResult>,
    doc_analysis: DashMap<String, Versioned<FileAnalysis>>,
    debounce_tokens: DashMap<String, tokio::sync::watch::Sender<()>>,
    documents: DashMap<String, (i32, String)>,
    effects_options: Mutex<LspOptions>,
    effect_cache: DashMap<String, Arc<CachedEffect>>,
    effect_jobs: DashMap<String, ()>,
    effect_failures: DashMap<String, Instant>,
    effect_worker: Arc<Semaphore>,
    effect_preview_worker: Arc<Semaphore>,
    effect_refresh_pending: AtomicBool,
    effect_fingerprint: Mutex<Option<String>>,
    inlay_refresh_supported: Mutex<bool>,
    lens_refresh_supported: Mutex<bool>,
}

impl State {
    fn new(client: Client, options: LspOptions) -> Self {
        Self {
            client,
            fuzzy_threshold: Mutex::new(0.85),
            spec_urls: Mutex::new(None),
            query_cache: DashMap::new(),
            doc_analysis: DashMap::new(),
            debounce_tokens: DashMap::new(),
            documents: DashMap::new(),
            effects_options: Mutex::new(options),
            effect_cache: DashMap::new(),
            effect_jobs: DashMap::new(),
            effect_failures: DashMap::new(),
            effect_worker: Arc::new(Semaphore::new(1)),
            effect_preview_worker: Arc::new(Semaphore::new(1)),
            effect_refresh_pending: AtomicBool::new(false),
            effect_fingerprint: Mutex::new(None),
            inlay_refresh_supported: Mutex::new(false),
            lens_refresh_supported: Mutex::new(false),
        }
    }

    async fn ensure_spec_urls(&self) -> Vec<SpecUrl> {
        let mut cached = self.spec_urls.lock().await;
        if let Some(ref urls) = *cached {
            return urls.clone();
        }
        let spec_entries = crate::spec_urls();
        let urls: Vec<SpecUrl> = spec_entries
            .iter()
            .map(|e| SpecUrl {
                spec: e.spec.clone(),
                base_url: e.base_url.clone(),
            })
            .collect();
        *cached = Some(urls.clone());
        urls
    }

    fn query_spec_cached(&self, spec: &str, anchor: &str) -> Option<QueryResult> {
        let key = format!("{spec}#{anchor}");
        if let Some(cached) = self.query_cache.get(&key) {
            return Some(cached.clone());
        }
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(crate::query_section(&key, None))
                .ok()
        })?;
        self.query_cache.insert(key, result.clone());
        Some(result)
    }

    async fn analyze_doc(&self, uri: &str, text: &str, version: i32) -> FileAnalysis {
        if let Some(cached) = self.doc_analysis.get(uri) {
            if cached.version == version {
                return cached.data.clone();
            }
        }

        let spec_urls = self.ensure_spec_urls().await;
        let threshold = *self.fuzzy_threshold.lock().await;
        let resolver = LspResolver { state: self };
        let analysis = analyze_file(text, &spec_urls, &resolver, threshold);

        self.doc_analysis.insert(
            uri.to_string(),
            Versioned {
                version,
                data: analysis.clone(),
            },
        );
        analysis
    }

    async fn publish_diagnostics(&self, uri: &str, text: &str, version: i32) {
        let analysis = self.analyze_doc(uri, text, version).await;
        let diagnostics = build_diagnostics(uri, &analysis);
        self.client
            .publish_diagnostics(
                uri.parse()
                    .unwrap_or_else(|_| Url::parse("file:///").unwrap()),
                diagnostics,
                None,
            )
            .await;
    }

    async fn effect_options(&self) -> Option<(EffectsOptions, usize)> {
        let options = self.effects_options.lock().await;
        options.effects_enabled.then(|| {
            (
                EffectsOptions {
                    rule_paths: options.rule_paths.clone(),
                    environment: options.environment.clone(),
                    ..EffectsOptions::default()
                },
                options.max_badges,
            )
        })
    }

    async fn effect_context(&self) -> Option<(EffectsOptions, usize, String)> {
        let (options, max_badges) = self.effect_options().await?;
        let fingerprint_options = options.clone();
        let fingerprint = tokio::task::spawn_blocking(move || {
            crate::effects::input_fingerprint(&fingerprint_options)
        })
        .await
        .ok()?
        .ok()?;
        let mut current = self.effect_fingerprint.lock().await;
        if current.as_deref() != Some(&fingerprint) {
            self.effect_cache.clear();
            self.effect_failures.clear();
            *current = Some(fingerprint.clone());
        }
        Some((options, max_badges, fingerprint))
    }

    fn effect_key(fingerprint: &str, subject: &SubjectSelector, expected_sha: &str) -> String {
        format!(
            "{}\0{}\0{}",
            fingerprint,
            expected_sha,
            serde_json::to_string(subject).unwrap_or_default()
        )
    }

    fn cached_effect_or_claim_job(
        &self,
        subject: &SubjectSelector,
        expected_sha: &str,
        fingerprint: &str,
    ) -> EffectLookup {
        let key = Self::effect_key(fingerprint, subject, expected_sha);
        if self
            .effect_failures
            .get(&key)
            .is_some_and(|failure| failure.elapsed() < EFFECT_RETRY_DELAY)
        {
            return EffectLookup::Pending;
        }
        cached_effect_or_claim_job(&self.effect_cache, &self.effect_jobs, key, expected_sha)
    }

    /// Schedule semantic work away from hover/inlay request paths. The coarse
    /// job key deduplicates requests while the engine computes the authoritative
    /// input fingerprint (catalog contents, environment and corpus generation).
    async fn schedule_effect(self: &Arc<Self>, job: EffectJob) {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let EffectJob {
                subject,
                expected_sha,
                document: (uri, version),
                options,
                fingerprint,
                key: job_key,
            } = job;
            let request = EffectsRequest {
                schema_version: crate::effects::model::EFFECTS_SCHEMA_VERSION,
                subject: subject.clone(),
                options: EffectsOptions {
                    mode: EffectsMode::Cached,
                    ..options.clone()
                },
                filter: None,
            };
            let worker_state = Arc::clone(&state);
            let worker_uri = uri.clone();
            let worker_fingerprint = fingerprint.clone();
            let work = run_effect_work(&state.effect_worker, move || {
                // A queued request may be obsolete by the time the worker is free.
                if !worker_state
                    .documents
                    .get(&worker_uri)
                    .is_some_and(|document| document.0 == version)
                {
                    return None;
                }
                if worker_state.effect_fingerprint.blocking_lock().as_deref()
                    != Some(&worker_fingerprint)
                {
                    return None;
                }
                let result = crate::effects::get_effect_summary(&request).map(|result| {
                    let catalog = crate::effects::default_catalog(&request.options.rule_paths).ok();
                    let markdown = editor_summary_markdown(&result, catalog.as_ref());
                    CachedEffect {
                        summary: result,
                        markdown,
                    }
                });
                Some(result)
            })
            .await;

            let mut refresh = false;
            match work {
                Ok(Some(Ok(result))) => {
                    if result.summary.subject.snapshot_sha == expected_sha {
                        let key = State::effect_key(&fingerprint, &subject, &expected_sha);
                        state.effect_cache.insert(key, Arc::new(result));
                        state.effect_failures.remove(&job_key);
                        refresh = true;
                    } else {
                        // The semantic run observed a newer spec snapshot. Drop the
                        // old markdown mapping and resolve it again before any effect
                        // can be attached to source comments.
                        let section_key = format!("{}#{}", subject.spec, subject.anchor);
                        state.query_cache.remove(&section_key);
                        if state
                            .documents
                            .get(&uri)
                            .is_some_and(|document| document.0 == version)
                        {
                            state.doc_analysis.remove(&uri);
                            let runtime = tokio::runtime::Handle::current();
                            let refresh_key = section_key.clone();
                            let refreshed = tokio::task::spawn_blocking(move || {
                                runtime.block_on(crate::query_section(&refresh_key, None))
                            })
                            .await;
                            if let Ok(Ok(query)) = refreshed {
                                state.query_cache.insert(section_key, query);
                                refresh = true;
                            }
                        }
                    }
                }
                Ok(None) => {}
                _ => {
                    // Refreshing after a failure resubmits the same job forever.
                    state
                        .effect_failures
                        .insert(job_key.clone(), Instant::now());
                }
            }
            state.effect_jobs.remove(&job_key);
            if refresh {
                // An async result is mapped back to the editor only if the same
                // document version is still open. The request path separately
                // verifies the spec snapshot before rendering it.
                let document_is_current = state
                    .documents
                    .get(&uri)
                    .is_some_and(|document| document.0 == version);
                if document_is_current {
                    state.schedule_effect_refresh();
                }
            }
        });
    }

    fn schedule_effect_refresh(self: &Arc<Self>) {
        if self.effect_refresh_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let state = Arc::clone(self);
        tokio::spawn(async move {
            // A file can have hundreds of step lookups. Refresh in batches,
            // rather than asking the editor to re-request the file for each one.
            tokio::time::sleep(Duration::from_millis(100)).await;
            state.effect_refresh_pending.store(false, Ordering::SeqCst);
            if !state.documents.is_empty() {
                if *state.inlay_refresh_supported.lock().await {
                    let _ = state.client.inlay_hint_refresh().await;
                }
                if *state.lens_refresh_supported.lock().await {
                    let _ = state.client.code_lens_refresh().await;
                }
            }
        });
    }
}

fn cached_effect_or_claim_job(
    cache: &DashMap<String, Arc<CachedEffect>>,
    jobs: &DashMap<String, ()>,
    key: String,
    expected_sha: &str,
) -> EffectLookup {
    if let Some(cached) = cache.get(&key) {
        if cached.summary.subject.snapshot_sha == expected_sha {
            return EffectLookup::Cached(Arc::clone(&cached));
        }
    }
    if claim_job(jobs, key.clone()) {
        EffectLookup::Claimed(key)
    } else {
        EffectLookup::Pending
    }
}

/// Spec resolver backed by the LSP's cached DB queries.
struct LspResolver<'a> {
    state: &'a State,
}

impl SpecResolver for LspResolver<'_> {
    fn resolve(&self, spec: &str, anchor: &str) -> Option<String> {
        self.state
            .query_spec_cached(spec, anchor)?
            .content
            .filter(|c| !c.is_empty())
    }
}

fn build_diagnostics(uri: &str, analysis: &FileAnalysis) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for scope in &analysis.scopes {
        for v in &scope.validations {
            if matches!(v.result, MatchResult::Exact | MatchResult::Fuzzy) {
                continue;
            }
            let step_label = step_label(&v.step.number);
            let msg = if v.result == MatchResult::NotFound {
                format!(
                    "Step {step_label}: not found in algorithm '{}'",
                    v.algo_anchor
                )
            } else {
                format!("Step {step_label}: text differs from spec")
            };
            let end_line = v.step.end_line.unwrap_or(v.step.line);
            let mut diag = Diagnostic {
                range: Range {
                    start: Position {
                        line: v.step.line as u32,
                        character: v.step.col_start as u32,
                    },
                    end: Position {
                        line: end_line as u32,
                        character: v.step.col_end as u32,
                    },
                },
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("webspec-lens".to_string()),
                message: msg,
                ..Default::default()
            };
            if !v.spec_text.is_empty() {
                diag.related_information = Some(vec![DiagnosticRelatedInformation {
                    location: Location {
                        uri: uri
                            .parse()
                            .unwrap_or_else(|_| Url::parse("file:///").unwrap()),
                        range: diag.range,
                    },
                    message: format!("Expected: {}", v.spec_text),
                }]);
            }
            diagnostics.push(diag);
        }
    }
    diagnostics
}

fn step_label(number: &[u32]) -> String {
    number
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// Find a step validation at the given cursor position.
fn find_validation_at_position(
    analysis: &FileAnalysis,
    line: usize,
    col: usize,
) -> Option<&StepValidation> {
    for scope in &analysis.scopes {
        for v in &scope.validations {
            if v.step.line != line {
                continue;
            }
            if col >= v.step.col_start && col <= v.step.col_end {
                return Some(v);
            }
        }
    }
    None
}

fn validation_is_uniquely_mapped(
    validations: &[StepValidation],
    validation: &StepValidation,
) -> bool {
    matches!(validation.result, MatchResult::Exact | MatchResult::Fuzzy)
        && validations
            .iter()
            .filter(|candidate| {
                candidate.algo_anchor == validation.algo_anchor
                    && candidate.step.number == validation.step.number
                    && matches!(candidate.result, MatchResult::Exact | MatchResult::Fuzzy)
            })
            .count()
            == 1
}

fn claim_job(jobs: &DashMap<String, ()>, key: String) -> bool {
    jobs.insert(key, ()).is_none()
}

fn append_effect_hover(mut markdown: String, effects: Option<&CachedEffect>) -> String {
    if let Some(effects) = effects.filter(|effects| !effects.markdown.is_empty()) {
        markdown.push_str("\n\n---\n\n");
        markdown.push_str(&effects.markdown);
    }
    markdown
}

pub struct Backend {
    state: Arc<State>,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PreparedEffectDocumentRequest {
    subject: SubjectSelector,
    category: Option<EditorCategory>,
    expected_sha: String,
    document_uri: String,
    document_version: i32,
    #[serde(default)]
    source_position: Option<Position>,
}

#[derive(serde::Serialize)]
struct EffectExplanationMarkdown {
    subject: crate::effects::model::Subject,
    markdown: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_details: Option<Vec<EditorEffectDetails>>,
}

async fn render_effect_document(
    result: ExplainEffectsResult,
    rule_paths: Vec<String>,
) -> Result<EffectExplanationMarkdown> {
    tokio::task::spawn_blocking(move || {
        let catalog = crate::effects::default_catalog(&rule_paths)
            .map_err(|error| Error::invalid_params(error.to_string()))?;
        Ok(EffectExplanationMarkdown {
            markdown: explanation_markdown(&result, Some(&catalog)),
            effect_details: None,
            subject: result.subject,
        })
    })
    .await
    .map_err(|_| Error::internal_error())?
}

impl Backend {
    async fn prepared_effect_document(
        &self,
        request: PreparedEffectDocumentRequest,
    ) -> Result<EffectExplanationMarkdown> {
        if !self
            .state
            .documents
            .get(&request.document_uri)
            .is_some_and(|doc| doc.0 == request.document_version)
        {
            return Err(Error::invalid_params(
                "The source changed; click the refreshed effects row.",
            ));
        }
        let (options, _) = self
            .state
            .effect_options()
            .await
            .ok_or_else(|| Error::invalid_params("effect analysis is disabled"))?;
        // Prepared clicks must not wait behind whole-file background lookups.
        // Keep them serialized independently; this path cannot run analysis.
        run_effect_work(&self.state.effect_preview_worker, move || {
            let result = crate::effects::service::prepared_effect_details(&EffectsRequest {
                schema_version: crate::effects::EFFECTS_SCHEMA_VERSION,
                subject: request.subject,
                options: options.clone(),
                filter: None,
            })
            .map_err(|error| Error::invalid_params(error.to_string()))?;
            if result.subject.snapshot_sha != request.expected_sha {
                return Err(Error::invalid_params(
                    "The spec changed; click the refreshed effects row.",
                ));
            }
            let catalog = crate::effects::default_catalog(&options.rule_paths)
                .map_err(|error| Error::invalid_params(error.to_string()))?;
            Ok(EffectExplanationMarkdown {
                markdown: editor_details_markdown(&result, request.category, Some(&catalog)),
                effect_details: Some(editor_effect_details(
                    &result,
                    request.category,
                    Some(&catalog),
                )),
                subject: result.subject,
            })
        })
        .await
        .map_err(|_| Error::internal_error())?
    }

    async fn effect_explanation_markdown(
        &self,
        request: ExplainEffectsRequest,
    ) -> Result<EffectExplanationMarkdown> {
        let rule_paths = request.options.rule_paths.clone();
        let result = self.effect_explanation(request).await?;
        render_effect_document(result, rule_paths).await
    }

    async fn effect_explanation_markdown_at_position(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<EffectExplanationMarkdown> {
        let (options, _) = self
            .state
            .effect_options()
            .await
            .ok_or_else(|| Error::invalid_params("effect analysis is disabled"))?;
        let result = self.effect_explanation_at_position(params).await?;
        render_effect_document(result, options.rule_paths).await
    }

    async fn effects(&self, request: EffectsRequest) -> Result<EffectSummaryResult> {
        run_effect_work(&self.state.effect_worker, move || {
            crate::effects::get_effect_summary(&request)
        })
        .await
        .map_err(|_| Error::internal_error())?
        .map_err(|error| Error::invalid_params(error.to_string()))
    }

    async fn effect_explanation(
        &self,
        request: ExplainEffectsRequest,
    ) -> Result<ExplainEffectsResult> {
        let result = run_effect_work(&self.state.effect_worker, move || {
            crate::effects::explain_effects(&request)
        })
        .await
        .map_err(|_| Error::internal_error())?
        .map_err(|error| Error::invalid_params(error.to_string()))?;
        // Explicit analysis may make previously unavailable summaries readable.
        let mut removed = false;
        self.state.effect_cache.retain(|_, cached| {
            let keep = !matches!(
                cached.summary.effects_status,
                EffectsStatus::Unavailable { .. }
            );
            removed |= !keep;
            keep
        });
        if removed {
            self.state.schedule_effect_refresh();
        }
        Ok(result)
    }

    async fn effect_explanation_at_position(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<ExplainEffectsResult> {
        let uri = params.text_document.uri.to_string();
        let (version, text) = self
            .state
            .documents
            .get(&uri)
            .map(|entry| entry.clone())
            .ok_or_else(|| Error::invalid_params("document is not open"))?;
        let analysis = self.state.analyze_doc(&uri, &text, version).await;
        let position = params.position;
        let validation = find_validation_at_position(
            &analysis,
            position.line as usize,
            position.character as usize,
        );
        let scope = if let Some(validation) = validation {
            analysis.scopes.iter().find(|scope| {
                scope
                    .validations
                    .iter()
                    .any(|candidate| std::ptr::eq(candidate, validation))
            })
        } else {
            find_url_at_position(
                &analysis.url_matches,
                position.line as usize,
                position.character as usize,
            )
            .and_then(|matched| {
                analysis.scopes.iter().find(|scope| {
                    scope.url_match.spec == matched.spec
                        && scope.url_match.anchor == matched.anchor
                        && scope.url_match.line == matched.line
                })
            })
        }
        .ok_or_else(|| Error::invalid_params("cursor is not on a spec URL or mapped step"))?;

        let step_path = validation
            .filter(|validation| validation_is_uniquely_mapped(&scope.validations, validation))
            .map(|validation| validation.step.number.clone());
        let subject = SubjectSelector {
            spec: scope.url_match.spec.clone(),
            anchor: scope.url_match.anchor.clone(),
            step_path,
            step_id: None,
            body_id: None,
        };
        let expected_sha = self
            .state
            .query_spec_cached(&subject.spec, &subject.anchor)
            .map(|query| query.sha)
            .ok_or_else(|| Error::invalid_params("spec section is unavailable"))?;
        let (options, _) = self
            .state
            .effect_options()
            .await
            .ok_or_else(|| Error::invalid_params("effect analysis is disabled"))?;
        let result = self
            .effect_explanation(ExplainEffectsRequest {
                schema_version: crate::effects::model::EFFECTS_SCHEMA_VERSION,
                subject: subject.clone(),
                options,
                filter: None,
                explanation: Default::default(),
            })
            .await?;
        if result.subject.snapshot_sha != expected_sha {
            self.state
                .query_cache
                .remove(&format!("{}#{}", subject.spec, subject.anchor));
            self.state.doc_analysis.remove(&uri);
            return Err(Error::invalid_params(
                "the spec snapshot changed; invoke Show Spec Effects again",
            ));
        }
        Ok(result)
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(opts) = &params.initialization_options {
            if let Some(threshold) = opts.get("fuzzyThreshold").and_then(|v| v.as_f64()) {
                if (0.0..=1.0).contains(&threshold) {
                    *self.state.fuzzy_threshold.lock().await = threshold;
                }
            }
            let mut effects = self.state.effects_options.lock().await;
            if let Some(enabled) = opts.get("effectsEnabled").and_then(|v| v.as_bool()) {
                effects.effects_enabled = enabled;
            }
            if let Some(max_badges) = opts.get("effectsMaxBadges").and_then(|v| v.as_u64()) {
                effects.max_badges = usize::try_from(max_badges).unwrap_or(usize::MAX);
            }
            if let Some(environment) = opts.get("effectsEnvironment").and_then(|v| v.as_str()) {
                if !environment.is_empty() {
                    effects.environment = environment.into();
                }
            }
            if let Some(paths) = opts.get("effectsRulePaths").and_then(|v| v.as_array()) {
                effects.rule_paths = paths
                    .iter()
                    .filter_map(|path| path.as_str().map(str::to_owned))
                    .collect();
            }
        }
        *self.state.lens_refresh_supported.lock().await = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.code_lens.as_ref())
            .and_then(|lenses| lenses.refresh_support)
            .unwrap_or(false);
        *self.state.inlay_refresh_supported.lock().await = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.inlay_hint.as_ref())
            .and_then(|hints| hints.refresh_support)
            .unwrap_or(false);

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.state.ensure_spec_urls().await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let text = params.text_document.text.clone();
        let version = params.text_document.version;
        self.state
            .documents
            .insert(uri.clone(), (version, text.clone()));
        self.state.publish_diagnostics(&uri, &text, version).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let version = params.text_document.version;

        if let Some(change) = params.content_changes.into_iter().last() {
            let text = change.text;
            self.state.documents.insert(uri.clone(), (version, text));

            // Cancel previous debounce
            if let Some((_, old_tx)) = self.state.debounce_tokens.remove(&uri) {
                let _ = old_tx.send(());
            }

            let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(());
            self.state.debounce_tokens.insert(uri.clone(), cancel_tx);

            let state = Arc::clone(&self.state);
            let uri_clone = uri;

            tokio::spawn(async move {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(DEBOUNCE_DELAY_MS)) => {
                        let (version, text) = match state.documents.get(&uri_clone) {
                            Some(entry) => entry.clone(),
                            None => return,
                        };
                        state.publish_diagnostics(&uri_clone, &text, version).await;
                    }
                    _ = cancel_rx.changed() => {
                        // Cancelled
                    }
                }
            });
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        if let Some((_, tx)) = self.state.debounce_tokens.remove(&uri) {
            let _ = tx.send(());
        }
        self.state.documents.remove(&uri);
        self.state.doc_analysis.remove(&uri);
        self.state
            .client
            .publish_diagnostics(params.text_document.uri, vec![], None)
            .await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let pos = params.text_document_position_params.position;

        let (version, text) = match self.state.documents.get(&uri) {
            Some(entry) => entry.clone(),
            None => return Ok(None),
        };

        let analysis = self.state.analyze_doc(&uri, &text, version).await;

        // Spec URL hover
        if let Some(url_match) = find_url_at_position(
            &analysis.url_matches,
            pos.line as usize,
            pos.character as usize,
        ) {
            if let Some(result) = self
                .state
                .query_spec_cached(&url_match.spec, &url_match.anchor)
            {
                let subject = SubjectSelector {
                    spec: result.spec.clone(),
                    anchor: result.anchor.clone(),
                    step_path: None,
                    step_id: None,
                    body_id: None,
                };
                let effects = if let Some((options, _, fingerprint)) =
                    self.state.effect_context().await
                {
                    match self
                        .state
                        .cached_effect_or_claim_job(&subject, &result.sha, &fingerprint)
                    {
                        EffectLookup::Cached(cached) => Some(cached),
                        EffectLookup::Claimed(job_key) => {
                            self.state
                                .schedule_effect(EffectJob {
                                    subject,
                                    expected_sha: result.sha.clone(),
                                    document: (uri.clone(), version),
                                    options,
                                    fingerprint,
                                    key: job_key,
                                })
                                .await;
                            None
                        }
                        EffectLookup::Pending => None,
                    }
                } else {
                    None
                };
                let markdown =
                    append_effect_hover(build_hover_content(&result), effects.as_deref());
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: Some(Range {
                        start: Position {
                            line: url_match.line as u32,
                            character: url_match.col_start as u32,
                        },
                        end: Position {
                            line: url_match.line as u32,
                            character: url_match.col_end as u32,
                        },
                    }),
                }));
            }
        }

        // Step comment hover
        if let Some(v) =
            find_validation_at_position(&analysis, pos.line as usize, pos.character as usize)
        {
            let label = step_label(&v.step.number);
            let mut md = match v.result {
                MatchResult::Exact => format!("**Step {label}** \u{2014} exact match"),
                MatchResult::Fuzzy => {
                    let mut s = format!("**Step {label}** \u{2014} fuzzy match");
                    if !v.spec_text.is_empty() {
                        s.push_str(&format!("\n\n**Spec:** {}", v.spec_text));
                    }
                    s
                }
                MatchResult::NotFound => {
                    format!("**Step {label}** \u{2014} not found in `{}`", v.algo_anchor)
                }
                MatchResult::Mismatch => {
                    let mut s = format!("**Step {label}** \u{2014} text differs from spec");
                    if !v.spec_text.is_empty() {
                        s.push_str(&format!("\n\n**Expected:** {}", v.spec_text));
                    }
                    s
                }
            };

            if let Some(scope) = analysis.scopes.iter().find(|scope| {
                scope
                    .validations
                    .iter()
                    .any(|candidate| std::ptr::eq(candidate, v))
            }) {
                if let Some(query) = self
                    .state
                    .query_spec_cached(&scope.url_match.spec, &scope.url_match.anchor)
                {
                    let uniquely_mapped = validation_is_uniquely_mapped(&scope.validations, v);
                    let subject = SubjectSelector {
                        spec: scope.url_match.spec.clone(),
                        anchor: scope.url_match.anchor.clone(),
                        step_path: uniquely_mapped.then(|| v.step.number.clone()),
                        step_id: None,
                        body_id: None,
                    };
                    let effects = if let Some((options, _, fingerprint)) =
                        self.state.effect_context().await
                    {
                        match self.state.cached_effect_or_claim_job(
                            &subject,
                            &query.sha,
                            &fingerprint,
                        ) {
                            EffectLookup::Cached(cached) => Some(cached),
                            EffectLookup::Claimed(job_key) => {
                                self.state
                                    .schedule_effect(EffectJob {
                                        subject,
                                        expected_sha: query.sha,
                                        document: (uri.clone(), version),
                                        options,
                                        fingerprint,
                                        key: job_key,
                                    })
                                    .await;
                                None
                            }
                            EffectLookup::Pending => None,
                        }
                    } else {
                        None
                    };
                    if effects.is_some() && !uniquely_mapped {
                        md.push_str(
                            "\n\n*Algorithm-level effects; this comment was not attributed to a spec step.*",
                        );
                    }
                    md = append_effect_hover(md, effects.as_deref());
                }
            }

            let end_line = v.step.end_line.unwrap_or(v.step.line);
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: md,
                }),
                range: Some(Range {
                    start: Position {
                        line: v.step.line as u32,
                        character: v.step.col_start as u32,
                    },
                    end: Position {
                        line: end_line as u32,
                        character: v.step.col_end as u32,
                    },
                }),
            }));
        }

        Ok(None)
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri.to_string();
        let (version, text) = match self.state.documents.get(&uri) {
            Some(entry) => entry.clone(),
            None => return Ok(None),
        };

        let analysis = self.state.analyze_doc(&uri, &text, version).await;
        let range_start = params.range.start.line as usize;
        let range_end = params.range.end.line as usize;
        let mut hints = Vec::new();
        let effects_context = self.state.effect_context().await;

        for scope in &analysis.scopes {
            for v in &scope.validations {
                if v.step.line < range_start || v.step.line > range_end {
                    continue;
                }

                let label_str = step_label(&v.step.number);
                let (hint_label, mut tooltip): (String, String) = match v.result {
                    MatchResult::Exact => (
                        " \u{2713}".into(),
                        format!("**Step {label_str}** \u{2014} exact match"),
                    ),
                    MatchResult::Fuzzy => {
                        let mut md = format!("**Step {label_str}** \u{2014} fuzzy match");
                        if !v.spec_text.is_empty() {
                            md.push_str(&format!("\n\n**Spec:** {}", v.spec_text));
                        }
                        (" \u{2713}".into(), md)
                    }
                    MatchResult::NotFound => (
                        " \u{26a0}".into(),
                        format!(
                            "**Step {label_str}** \u{2014} not found in `{}`",
                            v.algo_anchor
                        ),
                    ),
                    MatchResult::Mismatch => {
                        let mut md =
                            format!("**Step {label_str}** \u{2014} text differs from spec");
                        if !v.spec_text.is_empty() {
                            md.push_str(&format!("\n\n**Expected:** {}", v.spec_text));
                        }
                        (" \u{26a0}".into(), md)
                    }
                };

                if validation_is_uniquely_mapped(&scope.validations, v) {
                    if let (Some((options, _, fingerprint)), Some(query)) = (
                        effects_context.as_ref(),
                        self.state
                            .query_spec_cached(&scope.url_match.spec, &scope.url_match.anchor),
                    ) {
                        let subject = SubjectSelector {
                            spec: scope.url_match.spec.clone(),
                            anchor: scope.url_match.anchor.clone(),
                            step_path: Some(v.step.number.clone()),
                            step_id: None,
                            body_id: None,
                        };
                        match self.state.cached_effect_or_claim_job(
                            &subject,
                            &query.sha,
                            fingerprint,
                        ) {
                            EffectLookup::Cached(effects) => {
                                if !effects.markdown.is_empty() {
                                    tooltip.push_str("\n\n---\n\n");
                                    tooltip.push_str(&effects.markdown);
                                }
                            }
                            EffectLookup::Claimed(job_key) => {
                                self.state
                                    .schedule_effect(EffectJob {
                                        subject,
                                        expected_sha: query.sha,
                                        document: (uri.clone(), version),
                                        options: options.clone(),
                                        fingerprint: fingerprint.clone(),
                                        key: job_key,
                                    })
                                    .await;
                            }
                            EffectLookup::Pending => {}
                        }
                    }
                }

                let end_line = v.step.end_line.unwrap_or(v.step.line);
                hints.push(InlayHint {
                    position: Position {
                        line: end_line as u32,
                        character: v.step.col_end as u32,
                    },
                    label: InlayHintLabel::String(hint_label),
                    kind: Some(match v.result {
                        MatchResult::Exact | MatchResult::Fuzzy => InlayHintKind::TYPE,
                        _ => InlayHintKind::PARAMETER,
                    }),
                    tooltip: Some(InlayHintTooltip::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: tooltip,
                    })),
                    padding_left: Some(true),
                    padding_right: None,
                    text_edits: None,
                    data: None,
                });
            }
        }

        Ok(if hints.is_empty() { None } else { Some(hints) })
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri.to_string();
        let (version, text) = match self.state.documents.get(&uri) {
            Some(entry) => entry.clone(),
            None => return Ok(None),
        };

        let analysis = self.state.analyze_doc(&uri, &text, version).await;
        let mut lenses = Vec::new();
        let effects_context = self.state.effect_context().await;

        for scope in &analysis.scopes {
            if let Some(cov) = &scope.coverage {
                let missing_labels: Vec<String> =
                    cov.missing.iter().map(|s| step_label(s)).collect();

                lenses.push(CodeLens {
                    range: Range {
                        start: Position {
                            line: scope.url_match.line as u32,
                            character: 0,
                        },
                        end: Position {
                            line: scope.url_match.line as u32,
                            character: 0,
                        },
                    },
                    command: Some(Command {
                        title: cov.summary(),
                        command: "webspecLens.showCoverage".to_string(),
                        arguments: Some(vec![
                            serde_json::Value::String(cov.anchor.clone()),
                            serde_json::Value::Number(serde_json::Number::from(cov.total_steps)),
                            serde_json::to_value(&missing_labels).unwrap_or_default(),
                        ]),
                    }),
                    data: None,
                });
            }
            if let (Some((options, max_categories, fingerprint)), Some(query)) = (
                effects_context.as_ref(),
                self.state
                    .query_spec_cached(&scope.url_match.spec, &scope.url_match.anchor),
            ) {
                if *max_categories == 0 {
                    continue;
                }
                for validation in &scope.validations {
                    if !validation_is_uniquely_mapped(&scope.validations, validation) {
                        continue;
                    }
                    let subject = SubjectSelector {
                        spec: scope.url_match.spec.clone(),
                        anchor: scope.url_match.anchor.clone(),
                        step_path: Some(validation.step.number.clone()),
                        step_id: None,
                        body_id: None,
                    };
                    match self
                        .state
                        .cached_effect_or_claim_job(&subject, &query.sha, fingerprint)
                    {
                        EffectLookup::Cached(cached) => {
                            let categories = editor_categories(&cached.summary);
                            let mut shown: Vec<_> = categories
                                .iter()
                                .take(*max_categories)
                                .copied()
                                .map(Some)
                                .collect();
                            if categories.len() > *max_categories {
                                shown.push(None);
                            }
                            // VS Code draws CodeLens above the anchored line: place it
                            // on the line following the entire (possibly multiline) comment.
                            let line = (validation.step.end_line.unwrap_or(validation.step.line)
                                + 1)
                            .min(text.lines().count().saturating_sub(1))
                                as u32;
                            for category in shown {
                                lenses.push(CodeLens {
                                    range: Range::new(
                                        Position::new(line, 0),
                                        Position::new(line, 0),
                                    ),
                                    command: Some(Command {
                                        title: category
                                            .map_or("All effects", EditorCategory::label)
                                            .into(),
                                        command: "webspecLens.showEffects".into(),
                                        arguments: Some(vec![serde_json::to_value(
                                            PreparedEffectDocumentRequest {
                                                subject: subject.clone(),
                                                category,
                                                expected_sha: query.sha.clone(),
                                                document_uri: uri.clone(),
                                                document_version: version,
                                                source_position: Some(Position::new(
                                                    validation.step.line as u32,
                                                    text.lines()
                                                        .nth(validation.step.line)
                                                        .and_then(|line| {
                                                            line.get(..validation.step.col_start)
                                                        })
                                                        .map_or(0, |prefix| {
                                                            prefix.encode_utf16().count()
                                                        })
                                                        as u32,
                                                )),
                                            },
                                        )
                                        .unwrap_or_default()]),
                                    }),
                                    data: None,
                                });
                            }
                        }
                        EffectLookup::Claimed(key) => {
                            self.state
                                .schedule_effect(EffectJob {
                                    subject,
                                    expected_sha: query.sha.clone(),
                                    document: (uri.clone(), version),
                                    options: options.clone(),
                                    fingerprint: fingerprint.clone(),
                                    key,
                                })
                                .await
                        }
                        EffectLookup::Pending => {}
                    }
                }
            }
        }

        Ok(if lenses.is_empty() {
            None
        } else {
            Some(lenses)
        })
    }
}

/// Start the LSP server on stdio.
pub async fn serve_stdio() {
    serve_stdio_with_options(LspOptions::default()).await;
}

/// Start the LSP server with CLI-provided catalog and environment options.
pub async fn serve_stdio_with_options(options: LspOptions) {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| Backend {
        state: Arc::new(State::new(client, options.clone())),
    })
    .custom_method(
        "webspec/preparedEffectDocument",
        Backend::prepared_effect_document,
    )
    .custom_method("webspec/effects", Backend::effects)
    .custom_method("webspec/effectExplanation", Backend::effect_explanation)
    .custom_method(
        "webspec/effectExplanationMarkdown",
        Backend::effect_explanation_markdown,
    )
    .custom_method(
        "webspec/effectExplanationMarkdownAtPosition",
        Backend::effect_explanation_markdown_at_position,
    )
    .custom_method(
        "webspec/effectExplanationAtPosition",
        Backend::effect_explanation_at_position,
    )
    .finish();
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::scanner::StepComment;
    use crate::effects::model::{Coverage, EffectSummary, Execution, Semantics, Subject};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn effect_worker_serializes_distinct_requests() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let worker = Arc::new(Semaphore::new(1));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut jobs = Vec::new();
        for _ in 0..12 {
            let worker = Arc::clone(&worker);
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            jobs.push(tokio::spawn(async move {
                run_effect_work(&worker, move || {
                    peak.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(5));
                    active.fetch_sub(1, Ordering::SeqCst);
                })
                .await
                .unwrap();
            }));
        }
        for job in jobs {
            job.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_request_does_not_release_running_effect_worker() {
        let worker = Arc::new(Semaphore::new(1));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task_worker = Arc::clone(&worker);
        let task = tokio::spawn(async move {
            run_effect_work(&task_worker, move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
            })
            .await
            .unwrap();
        });
        ready.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(worker.try_acquire().is_err());
        release.send(()).unwrap();
        let permit = tokio::time::timeout(Duration::from_secs(2), worker.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
    }

    fn validation(result: MatchResult, number: &[u32]) -> StepValidation {
        StepValidation {
            step: StepComment {
                line: 1,
                col_start: 0,
                col_end: 10,
                indent: 0,
                number: number.to_vec(),
                text: "step".into(),
                end_line: None,
            },
            result,
            spec_text: "step".into(),
            algo_anchor: "algorithm".into(),
        }
    }

    fn cached_effect(snapshot_sha: &str) -> CachedEffect {
        CachedEffect {
            summary: EffectSummaryResult {
                schema_version: crate::effects::model::EFFECTS_SCHEMA_VERSION,
                subject: Subject {
                    spec: "TEST".into(),
                    anchor: "algorithm".into(),
                    snapshot_sha: snapshot_sha.into(),
                    step_id: None,
                    step_path: Some(vec![1]),
                    body_id: None,
                },
                effects: vec![],
                effects_status: EffectsStatus::Ready {
                    semantics: Semantics::May,
                    coverage: Coverage::Complete,
                    issues: vec![],
                    omitted: 0,
                    analysis_id: "an_test".into(),
                },
                defined_bodies: vec![],
                issues: vec![],
                input_manifest: None,
            },
            markdown: "cached".into(),
        }
    }

    #[test]
    fn only_unique_exact_or_fuzzy_steps_receive_effect_attribution() {
        let exact = validation(MatchResult::Exact, &[1]);
        assert!(validation_is_uniquely_mapped(
            std::slice::from_ref(&exact),
            &exact
        ));

        let fuzzy = validation(MatchResult::Fuzzy, &[2]);
        assert!(validation_is_uniquely_mapped(
            std::slice::from_ref(&fuzzy),
            &fuzzy
        ));

        let mismatch = validation(MatchResult::Mismatch, &[3]);
        assert!(!validation_is_uniquely_mapped(
            std::slice::from_ref(&mismatch),
            &mismatch
        ));

        let duplicate = vec![exact.clone(), validation(MatchResult::Fuzzy, &[1])];
        assert!(!validation_is_uniquely_mapped(&duplicate, &duplicate[0]));
    }

    #[test]
    fn job_claims_are_deduplicated_until_completion() {
        let jobs = DashMap::new();
        assert!(claim_job(&jobs, "same-input-and-scope".into()));
        assert!(!claim_job(&jobs, "same-input-and-scope".into()));
        jobs.remove("same-input-and-scope");
        assert!(claim_job(&jobs, "same-input-and-scope".into()));
    }

    #[test]
    fn cache_hit_does_not_claim_a_job_that_can_trigger_refresh() {
        let cache = DashMap::new();
        let jobs = DashMap::new();
        cache.insert("key".into(), Arc::new(cached_effect("sha")));

        let lookup = cached_effect_or_claim_job(&cache, &jobs, "key".into(), "sha");

        assert!(matches!(lookup, EffectLookup::Cached(_)));
        assert!(jobs.is_empty());
    }

    #[test]
    fn cache_miss_claims_one_job_and_deduplicates_followup_requests() {
        let cache = DashMap::new();
        let jobs = DashMap::new();

        let first = cached_effect_or_claim_job(&cache, &jobs, "key".into(), "sha");
        let second = cached_effect_or_claim_job(&cache, &jobs, "key".into(), "sha");

        assert!(matches!(first, EffectLookup::Claimed(ref key) if key == "key"));
        assert!(matches!(second, EffectLookup::Pending));
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn stale_cached_snapshot_is_recomputed() {
        let cache = DashMap::new();
        let jobs = DashMap::new();
        cache.insert("key".into(), Arc::new(cached_effect("old-sha")));

        let lookup = cached_effect_or_claim_job(&cache, &jobs, "key".into(), "new-sha");

        assert!(matches!(lookup, EffectLookup::Claimed(ref key) if key == "key"));
        assert!(jobs.contains_key("key"));
    }

    #[test]
    fn cache_keys_include_fingerprint_and_spec_snapshot() {
        let subject = SubjectSelector {
            spec: "TEST".into(),
            anchor: "algorithm".into(),
            step_path: Some(vec![1]),
            step_id: None,
            body_id: None,
        };
        let key = State::effect_key("inputs-a", &subject, "sha-a");
        assert_ne!(key, State::effect_key("inputs-b", &subject, "sha-a"));
        assert_ne!(key, State::effect_key("inputs-a", &subject, "sha-b"));
    }

    #[test]
    fn partial_results_only_show_positive_concise_hints() {
        let mut result = EffectSummaryResult {
            schema_version: crate::effects::model::EFFECTS_SCHEMA_VERSION,
            subject: Subject {
                spec: "TEST".into(),
                anchor: "algorithm".into(),
                snapshot_sha: "sha".into(),
                step_id: None,
                step_path: Some(vec![1]),
                body_id: None,
            },
            effects: (0..5)
                .map(|i| EffectSummary {
                    id: format!("ef_{i}"),
                    kind: "script.invoke".into(),
                    params: Default::default(),
                    execution: vec![Execution::Inline],
                    location: None,
                    other_locations: Vec::new(),
                    additional_locations: 0,
                })
                .collect(),
            effects_status: EffectsStatus::Ready {
                semantics: Semantics::May,
                coverage: Coverage::Partial,
                issues: vec![],
                omitted: 0,
                analysis_id: "an_test".into(),
            },
            defined_bodies: vec![],
            issues: vec![],
            input_manifest: None,
        };
        let categories = editor_categories(&result);
        assert_eq!(categories, vec![EditorCategory::Script]);
        let tooltip = editor_summary_markdown(&result, None);
        assert!(!tooltip.contains("webspec-index"));
        assert!(!tooltip.contains("src-"));
        result.effects.clear();
        assert!(editor_categories(&result).is_empty());
    }
}
