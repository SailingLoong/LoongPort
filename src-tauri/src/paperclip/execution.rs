//! One durable claim; original router, price, balance and usage owners perform the work.
use super::contract::{self, Task};
use super::*;
#[cfg(any(unix, test))]
use crate::{database::Database, services::usage_stats::RawRequestLog};
use crate::{
    provider::Provider,
    proxy::{
        usage::{calculator::CostCalculator, logger::UsageLogger, parser::TokenUsage},
        ProxyError,
    },
    services::proxy::ProxyService,
};
use rust_decimal::{prelude::ToPrimitive, Decimal};
use std::str::FromStr;
#[cfg(any(unix, test))]
use std::{collections::HashMap, future::Future};

#[derive(Clone)]
pub(crate) struct RouteBinding {
    pub identity: crate::services::proxy::RequestIdentity,
    owner: String,
    app: String,
    revision: String,
    chain: Vec<String>,
    task: Task,
    execution_id: String,
    state: Arc<Mutex<AttemptState>>,
}
#[derive(Default)]
struct AttemptState {
    sent: bool,
    evidence: Option<SendEvidence>,
    #[cfg(any(unix, test))]
    usage: Option<RawRequestLog>,
    #[cfg(any(unix, test))]
    rejection: Option<String>,
    #[cfg(any(unix, test))]
    usage_trusted: bool,
}
#[derive(Clone)]
pub(crate) struct SendEvidence {
    pub provider_id: String,
    pub model: String,
    pub price_hash: String,
    pub multiplier: Decimal,
    pub input_tokens: u64,
    #[cfg(test)]
    pub estimate_micros: u64,
}
fn rejected(code: &str) -> ProxyError {
    ProxyError::ConfigError(format!("paperclip.{code}"))
}
pub(crate) fn now_ms() -> Result<i64, AppError> {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| unavailable())?
            .as_millis(),
    )
    .map_err(|_| unavailable())
}
pub(crate) fn cost_micros(cost: Decimal) -> Result<u64, AppError> {
    if cost.is_sign_negative() {
        return Err(unavailable());
    }
    let micros = cost
        .checked_mul(Decimal::from(1_000_000))
        .ok_or_else(unavailable)?
        .ceil()
        .to_u64()
        .ok_or_else(unavailable)?;
    if micros > contract::MAX_SAFE_INTEGER {
        return Err(unavailable());
    }
    Ok(micros)
}
fn balance_micros(balance: f64) -> Result<u64, AppError> {
    if !balance.is_finite() || balance < 0.0 {
        return Err(unavailable());
    }
    let exact = Decimal::from_f64_retain(balance).ok_or_else(unavailable)?;
    let micros = exact
        .checked_mul(Decimal::from(1_000_000))
        .ok_or_else(unavailable)?
        .floor()
        .to_u64()
        .ok_or_else(unavailable)?;
    if micros > contract::MAX_SAFE_INTEGER {
        return Err(unavailable());
    }
    Ok(micros)
}
// Keep the original proxy session normalizer as the sole identity format owner.
pub(super) fn usage_session_for_binding(id: &str) -> Result<String, AppError> {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "session_id",
        axum::http::HeaderValue::from_str(&format!("paperclip-{id}")).map_err(|_| unavailable())?,
    );
    let session = crate::proxy::extract_session_id(&headers, &Value::Null, "codex");
    if !session.client_provided {
        return Err(unavailable());
    }
    Ok(session.session_id)
}

impl RouteBinding {
    #[cfg(any(unix, test))]
    fn new(
        owner: &Owner,
        s: &Scope,
        task: Task,
        conn: &rusqlite::Connection,
    ) -> Result<Self, AppError> {
        let execution_id = contract::binding(s, owner.app.as_str(), &task)?["bindingId"]
            .as_str()
            .ok_or_else(unavailable)?
            .to_owned();
        Ok(Self {
            execution_id,
            identity: owner
                .state
                .proxy_service
                .request_identity(owner.app.as_str())?,
            owner: owner.owner.clone(),
            app: owner.app.as_str().into(),
            revision: s.snapshot_revision.clone(),
            chain: crate::proxy::application_routing::chain_ids_on(conn, owner.app.as_str())?,
            task,
            state: Arc::new(Mutex::new(AttemptState::default())),
        })
    }
    pub(crate) fn has_sent(&self) -> bool {
        self.state.lock().map(|s| s.sent).unwrap_or(true)
    }
    pub(crate) fn check_route(
        &self,
        service: &ProxyService,
        app: &str,
        providers: &[Provider],
    ) -> Result<(), ProxyError> {
        if self.app != app
            || app != "codex"
            || !service
                .request_identity_is_current(app, Some(&self.identity))
                .unwrap_or(false)
        {
            return Err(rejected("identity_changed"));
        }
        let conn = service
            .database()
            .conn
            .lock()
            .map_err(|_| rejected("database_unavailable"))?;
        self.check_on(&conn)
            .map_err(|_| rejected("applied_chain_changed"))?;
        if providers.is_empty() || providers.iter().any(|p| !self.chain.contains(&p.id)) {
            return Err(rejected("candidate_not_authorized"));
        }
        Ok(())
    }
    fn check_on(&self, conn: &rusqlite::Connection) -> Result<(), AppError> {
        let s = snapshot::snapshot_on_connection(
            conn,
            &self.app,
            &self.owner,
            self.identity.fingerprint(),
        )?;
        if s["revision"] != self.revision {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(crate) fn before_send(
        &self,
        service: &ProxyService,
        app: &str,
        provider: &Provider,
        body: &Value,
    ) -> Result<(), ProxyError> {
        #[cfg(not(any(unix, test)))]
        {
            self.before_send_checked(service, app, provider, body)
        }
        #[cfg(any(unix, test))]
        {
            let result = self.before_send_checked(service, app, provider, body);
            if let Err(ProxyError::ConfigError(code)) = &result {
                if let Some(code) = code.strip_prefix("paperclip.") {
                    if let Ok(mut attempt) = self.state.lock() {
                        if !attempt.sent {
                            attempt.rejection = Some(code.to_owned());
                        }
                    }
                }
            }
            result
        }
    }
    fn before_send_checked(
        &self,
        service: &ProxyService,
        app: &str,
        provider: &Provider,
        body: &Value,
    ) -> Result<(), ProxyError> {
        self.check_route(service, app, std::slice::from_ref(provider))?;
        let mut attempt = self
            .state
            .lock()
            .map_err(|_| rejected("attempt_unavailable"))?;
        if attempt.sent {
            return Err(rejected("outcome_unresolved_no_replay"));
        }
        if self.task.budget.currency != "USD" {
            return Err(rejected("currency_unavailable"));
        }
        if body.get("stream").and_then(Value::as_bool) != Some(false)
            || body.get("store").and_then(Value::as_bool) != Some(false)
        {
            return Err(rejected("bounded_responses_required"));
        }
        // Provider overrides and transforms must not replace the bound objective
        // or enable tools/side effects outside this bounded text-only task.
        let expected_input =
            json!([{"role":"user","content":[{"type":"input_text","text":self.task.input}]}]);
        if body.get("input") != Some(&expected_input)
            || body.as_object().is_none_or(|object| {
                object.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "model" | "input" | "stream" | "store" | "max_output_tokens"
                    )
                })
            })
        {
            return Err(rejected("task_body_changed"));
        }
        let max_output = body
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .filter(|n| *n > 0 && *n <= u64::from(self.task.max_output_tokens))
            .ok_or_else(|| rejected("output_allowance_changed"))?;
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| rejected("model_unavailable"))?;
        let input_tokens = crate::proxy::local_tokens::count_request_tokens(body);
        let input = u32::try_from(input_tokens)
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| rejected("input_estimate_unavailable"))?;
        let now = now_ms().map_err(|_| rejected("clock_unavailable"))?;
        let db = service.database();
        let conn = db
            .conn
            .lock()
            .map_err(|_| rejected("database_unavailable"))?;
        self.check_on(&conn)
            .map_err(|_| rejected("applied_chain_changed"))?;
        let origin = provider
            .website_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| rejected("account_unavailable"))?;
        let account = provider
            .meta
            .as_ref()
            .and_then(|m| m.loongport_account_id)
            .ok_or_else(|| rejected("account_unavailable"))?;
        let (balance, observed) =
            crate::relay::balance::cached_site_balance_on(&conn, &(origin.clone(), account))
                .map_err(|_| rejected("balance_unavailable"))?
                .ok_or_else(|| rejected("balance_unavailable"))?;
        let age = (now / 1000)
            .checked_sub(observed)
            .filter(|age| *age >= 0 && *age <= crate::relay::balance::SITE_BALANCE_TTL_SECS)
            .ok_or_else(|| rejected("balance_stale"))?;
        let _ = age;
        let available = balance_micros(balance.ok_or_else(|| rejected("balance_unavailable"))?)
            .map_err(|_| rejected("balance_unavailable"))?;
        let price = crate::services::model_pricing::observed_pricing_on(db, &conn, model, now)
            .map_err(|_| rejected("pricing_unavailable"))?;
        if price.observation.currency != "USD" {
            return Err(rejected("currency_unavailable"));
        }
        let (multiplier, source) =
            UsageLogger::resolve_pricing_config_checked_on(&conn, provider, app)
                .map_err(|_| rejected("pricing_configuration_unavailable"))?;
        if source != crate::database::PRICING_SOURCE_REQUEST {
            return Err(rejected("response_model_pricing_unavailable"));
        }
        let usage = TokenUsage {
            input_tokens: input,
            output_tokens: u32::try_from(max_output)
                .map_err(|_| rejected("output_allowance_changed"))?,
            ..Default::default()
        };
        let cost = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            CostCalculator::calculate_for_app(app, &usage, &price.pricing, multiplier)
        }))
        .map_err(|_| rejected("estimate_overflow"))?;
        let estimate = cost_micros(cost.total_cost).map_err(|_| rejected("estimate_overflow"))?;
        if estimate > self.task.budget.limit_micros {
            return Err(rejected("estimate_exceeds_budget"));
        }
        if estimate > available {
            return Err(rejected("balance_insufficient"));
        }
        if !service
            .request_identity_is_current(app, Some(&self.identity))
            .unwrap_or(false)
        {
            return Err(rejected("identity_changed"));
        }
        // All checks precede this irreversible in-memory send claim. The durable
        // run was already accepted; a crash here never permits replay.
        attempt.sent = true;
        #[cfg(any(unix, test))]
        {
            attempt.rejection = None;
        }
        attempt.evidence = Some(SendEvidence {
            provider_id: provider.id.clone(),
            model: model.into(),
            price_hash: price.observation.row_hash,
            multiplier,
            input_tokens,
            #[cfg(test)]
            estimate_micros: estimate,
        });
        Ok(())
    }
    pub(crate) fn evidence(&self) -> Result<SendEvidence, AppError> {
        self.state
            .lock()
            .map_err(|_| unavailable())?
            .evidence
            .clone()
            .ok_or_else(unavailable)
    }
    pub(crate) fn record_usage(
        &self,
        service: &ProxyService,
        calculated: crate::proxy::usage::logger::CalculatedUsage,
    ) -> Result<(), AppError> {
        let row = calculated.row;
        let evidence = self.evidence()?;
        if row.provider_id != evidence.provider_id
            || row.app_type != self.app
            || row.pricing_model.as_deref() != Some(evidence.model.as_str())
            || row.request_model.as_deref() != Some(self.task.model.as_str())
            || row.session_id.as_deref()
                != Some(usage_session_for_binding(&self.execution_id)?.as_str())
            || row
                .cost_multiplier
                .as_deref()
                .and_then(|v| Decimal::from_str(v).ok())
                != Some(evidence.multiplier)
        {
            return Err(unavailable());
        }
        // Preserve the original confirmed usage reference independently of
        // whether its price observation still qualifies budget completion.
        #[cfg(any(unix, test))]
        {
            self.state.lock().map_err(|_| unavailable())?.usage = Some(row);
        }
        // Compare the very row used for calculation, not just a later current
        // lookup: A -> B (calculated) -> A must not regain completion authority.
        if calculated.price_hash != evidence.price_hash {
            return Err(unavailable());
        }
        let price = crate::services::model_pricing::observed_pricing(
            service.database(),
            &evidence.model,
            now_ms()?,
        )?;
        if price.observation.row_hash != evidence.price_hash || price.observation.currency != "USD"
        {
            return Err(unavailable());
        }
        #[cfg(any(unix, test))]
        {
            self.state.lock().map_err(|_| unavailable())?.usage_trusted = true;
        }
        Ok(())
    }
    #[cfg(any(unix, test))]
    pub(crate) fn session_id(&self) -> String {
        format!("paperclip-{}", self.execution_id)
    }
    #[cfg(any(unix, test))]
    fn usage(&self) -> Result<RawRequestLog, AppError> {
        self.state
            .lock()
            .map_err(|_| unavailable())?
            .usage
            .clone()
            .ok_or_else(unavailable)
    }
    #[cfg(test)]
    pub(crate) fn for_test(
        _db: Arc<Database>,
        service: &ProxyService,
        task: Task,
        provider_ids: Vec<String>,
    ) -> Result<Self, AppError> {
        let identity = service.request_identity("codex")?;
        let c = service.database().conn.lock()?;
        let s =
            snapshot::snapshot_on_connection(&c, "codex", "test-owner", identity.fingerprint())?;
        let chain = crate::proxy::application_routing::chain_ids_on(&c, "codex")?;
        if chain != provider_ids {
            return Err(unavailable());
        }
        Ok(Self {
            execution_id: uuid::Uuid::new_v4().to_string(),
            identity,
            owner: "test-owner".into(),
            app: "codex".into(),
            revision: s["revision"].as_str().ok_or_else(unavailable)?.into(),
            chain,
            task,
            state: Arc::new(Mutex::new(AttemptState::default())),
        })
    }
}

#[cfg(any(unix, test))]
#[derive(Default)]
pub(super) struct Executions {
    pub(super) tasks: Mutex<HashMap<String, Worker>>,
}
#[cfg(any(unix, test))]
pub(super) struct Worker {
    cancel: Arc<tokio::sync::Notify>,
    thread: Option<thread::JoinHandle<()>>,
    finished: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Value>>>,
}
#[cfg(any(unix, test))]
impl Executions {
    pub(super) fn stop(&self) -> Result<(), AppError> {
        let mut tasks = std::mem::take(&mut *self.tasks.lock().map_err(|_| unavailable())?);
        for w in tasks.values() {
            w.cancel.notify_one();
        }
        let mut failed = false;
        for w in tasks.values_mut() {
            if let Some(t) = w.thread.take() {
                if t.join().is_err() {
                    failed = true;
                }
            }
        }
        if failed {
            Err(unavailable())
        } else {
            Ok(())
        }
    }
}
#[cfg(any(unix, test))]
impl Owner {
    pub(super) fn recover_execution(&self) -> Result<(), AppError> {
        let c = self.db_admission(Instant::now() + REQUEST_BUDGET)?;
        c.execute("UPDATE paperclip_run_bindings SET phase='unknown' WHERE owner=?1 AND app=?2 AND phase='accepted'",params![self.owner,self.app.as_str()]).map_err(|_|unavailable())?;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn bind_execution(&self, v: Value) -> Result<Value, AppError> {
        self.bind_execution_until(v, Instant::now() + REQUEST_BUDGET)
    }
    pub(super) fn bind_execution_until(
        &self,
        v: Value,
        deadline: Instant,
    ) -> Result<Value, AppError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            scope: Scope,
            task: Task,
        }
        let input: Input = serde_json::from_value(v).map_err(|_| unavailable())?;
        let s = self.scope(serde_json::to_value(input.scope).map_err(|_| unavailable())?)?;
        let b = contract::binding(&s, self.app.as_str(), &input.task)?;
        let encoded = serde_json::to_string(&s).map_err(|_| unavailable())?;
        let data = b.to_string();
        use futures::FutureExt;
        let _sync = self.sync_admission(deadline)?;
        let _app = self
            .state
            .proxy_service
            .lock_switch_for_app(self.app.as_str())
            .now_or_never()
            .ok_or_else(unavailable)?;
        let mut c = self.db_admission(deadline)?;
        let tx = c.transaction().map_err(|_| unavailable())?;
        if let Some(old) = self.saved(&tx, &s)? {
            if old.scope != encoded || old.binding != data {
                return Err(unavailable());
            }
            return Ok(b);
        }
        self.current(&tx, &s)?;
        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM paperclip_run_bindings", [], |r| {
                r.get(0)
            })
            .map_err(|_| unavailable())?;
        if count >= 1024 {
            return Err(unavailable());
        }
        tx.execute("INSERT INTO paperclip_run_bindings(owner,app,company,run,scope,binding,phase) VALUES(?1,?2,?3,?4,?5,?6,'bound')",params![self.owner,self.app.as_str(),s.company_id,s.run_id,encoded,data]).map_err(|_|unavailable())?;
        self.admitted(deadline)?;
        tx.commit().map_err(|_| unavailable())?;
        Ok(b)
    }
    fn execution_scope(&self, b: &Value) -> Result<Scope, AppError> {
        let s = contract::scope_in_binding(b)?;
        if s.owner_id != self.owner {
            return Err(unavailable());
        }
        contract::validate_binding(b, &s, self.app.as_str())?;
        Ok(s)
    }
    pub(crate) fn execution_result(&self, b: Value) -> Result<Value, AppError> {
        self.execution_result_until(b, Instant::now() + REQUEST_BUDGET)
    }
    pub(super) fn execution_result_until(
        &self,
        b: Value,
        deadline: Instant,
    ) -> Result<Value, AppError> {
        let s = self.execution_scope(&b)?;
        let id = b["bindingId"].as_str().ok_or_else(unavailable)?;
        let tasks = self
            .control
            .executions
            .tasks
            .lock()
            .map_err(|_| unavailable())?;
        let finished = tasks
            .get(id)
            .is_some_and(|w| w.finished.load(Ordering::Acquire));
        let in_memory = tasks
            .get(id)
            .and_then(|w| w.result.lock().ok().and_then(|r| r.clone()));
        let c = self.db_admission(deadline)?;
        let row = self.saved(&c, &s)?.ok_or_else(unavailable)?;
        if serde_json::from_str::<Value>(&row.binding).map_err(|_| unavailable())? != b {
            return Err(unavailable());
        }
        if let Some(raw) = row.receipt {
            let receipt: Value = serde_json::from_str(&raw).map_err(|_| unavailable())?;
            if !receipt["usage"].is_null() {
                let reference = receipt["usage"]["requestId"]
                    .as_str()
                    .ok_or_else(unavailable)?;
                let usage =
                    Database::get_request_detail_raw_on(&c, reference)?.ok_or_else(|| {
                        AppError::Config("paperclip.usage_evidence_unavailable".into())
                    })?;
                let digest = contract::digest(
                    b"paperclip-original-usage-v3\0",
                    &serde_json::to_vec(&usage).map_err(|_| unavailable())?,
                );
                if receipt["usage"]["digest"] != digest
                    || usage.app_type != self.app.as_str()
                    || usage.session_id.as_deref() != Some(usage_session_for_binding(id)?.as_str())
                {
                    return Err(AppError::Config(
                        "paperclip.usage_evidence_unavailable".into(),
                    ));
                }
            }

            if let Some(result) = in_memory {
                if contract::durable_result(&result) == receipt {
                    return Ok(result);
                }
            }
            // The worker commits its durable receipt before publishing the
            // in-memory response. A live worker in that gap has not lost output.
            // Only completed/absent workers qualify for crash recovery semantics.
            if tasks.contains_key(id) && !finished {
                return Ok(contract::outcome(&b, "accepted"));
            }
            return Ok(contract::recovered_result(&receipt));
        }
        match row.phase.as_str() {
            "bound" => Err(unavailable()),
            "accepted" if finished => {
                c.execute("UPDATE paperclip_run_bindings SET phase='unknown' WHERE owner=?1 AND app=?2 AND company=?3 AND run=?4 AND phase='accepted'",params![self.owner,self.app.as_str(),s.company_id,s.run_id]).map_err(|_|unavailable())?;
                Ok(contract::outcome(&b, "unknown"))
            }
            "accepted" => Ok(contract::outcome(&b, "accepted")),
            "unknown" => Ok(contract::outcome(&b, "unknown")),
            _ => Err(unavailable()),
        }
    }
    #[cfg(unix)]
    pub(super) fn cancel_execution_until(
        &self,
        b: Value,
        deadline: Instant,
    ) -> Result<Value, AppError> {
        let r = self.execution_result_until(b.clone(), deadline)?;
        if r["status"] == "accepted" {
            let tasks = self
                .control
                .executions
                .tasks
                .lock()
                .map_err(|_| unavailable())?;
            if let Some(worker) = tasks.get(b["bindingId"].as_str().ok_or_else(unavailable)?) {
                worker.cancel.notify_one();
            }
        }
        Ok(r)
    }
    #[cfg(test)]
    pub(crate) fn start_execution(&self, v: Value) -> Result<Value, AppError> {
        self.start_execution_until(v, Instant::now() + REQUEST_BUDGET)
    }
    pub(super) fn start_execution_until(
        &self,
        v: Value,
        deadline: Instant,
    ) -> Result<Value, AppError> {
        let owner = self.clone();
        self.start_execution_with_until(
            v,
            move |task, route| async move { owner.request_original_response(task, route).await },
            deadline,
        )
    }
    pub(super) fn start_execution_with_until<F, Fut>(
        &self,
        v: Value,
        runner: F,
        deadline: Instant,
    ) -> Result<Value, AppError>
    where
        F: FnOnce(Task, RouteBinding) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Value, AppError>> + Send + 'static,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            binding: Value,
            task: Task,
        }
        let input: Input = serde_json::from_value(v).map_err(|_| unavailable())?;
        let b = input.binding;
        let s = self.execution_scope(&b)?;
        let task = contract::task_for_start(&b, &s, self.app.as_str(), input.task)?;
        let mut workers = self
            .control
            .executions
            .tasks
            .lock()
            .map_err(|_| unavailable())?;
        let runtime;
        let route;
        {
            use futures::FutureExt;
            let _sync = self.sync_admission(deadline)?;
            let _app = self
                .state
                .proxy_service
                .lock_switch_for_app(self.app.as_str())
                .now_or_never()
                .ok_or_else(unavailable)?;
            let mut c = self.db_admission(deadline)?;
            let tx = c.transaction().map_err(|_| unavailable())?;
            let row = self.saved(&tx, &s)?.ok_or_else(unavailable)?;
            if serde_json::from_str::<Value>(&row.binding).map_err(|_| unavailable())? != b {
                return Err(unavailable());
            }
            if row.phase != "bound" {
                drop(tx);
                drop(c);
                drop(_sync);
                drop(workers);
                return self.execution_result_until(b, deadline);
            }
            runtime = self.runtime.clone().ok_or_else(unavailable)?;
            self.current(&tx, &s)?;
            route = RouteBinding::new(self, &s, task.clone(), &tx)?;
            tx.execute("UPDATE paperclip_run_bindings SET phase='accepted' WHERE owner=?1 AND app=?2 AND company=?3 AND run=?4 AND phase='bound'",params![self.owner,self.app.as_str(),s.company_id,s.run_id]).map_err(|_|unavailable())?;
            self.admitted(deadline)?;
            tx.commit().map_err(|_| unavailable())?;
        }
        fault("execution-claimed");
        let cancel = Arc::new(tokio::sync::Notify::new());
        let signal = cancel.clone();
        let owner = self.clone();
        let binding = b.clone();
        let id = b["bindingId"].as_str().ok_or_else(unavailable)?.to_owned();
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let result = Arc::new(Mutex::new(None));
        let output = result.clone();
        let attempt = route.clone();
        let worker=thread::Builder::new().name("paperclip-request".into()).spawn(move||{
            struct Finished(Arc<AtomicBool>);impl Drop for Finished{fn drop(&mut self){self.0.store(true,Ordering::Release);}}let _done=Finished(done);
            let outcome=if owner.control.stop.load(Ordering::Acquire){None}else{runtime.block_on(async {tokio::select!{biased;_=signal.notified()=>None,r=tokio::time::timeout(Duration::from_millis(task.timeout_ms),runner(task.clone(),route))=>r.ok().and_then(Result::ok)}})};
            let mut value=contract::outcome(&binding,"unknown");value["providerWorkStarted"]=json!(attempt.has_sent());
            if let Some(response)=outcome { if let Some(text)=response["output"].as_str().filter(|s|!s.is_empty()&&s.len()<=16384){value["output"]=json!(text);value["outputDigest"]=json!(contract::digest(b"paperclip-output-v3\0",text.as_bytes()));}}
            if let Ok(row)=attempt.usage(){
                value["usage"]=json!({"requestId":row.request_id,"digest":contract::digest(b"paperclip-original-usage-v3\0",&serde_json::to_vec(&row).unwrap_or_default()),"currency":"USD"});
                if !attempt.state.lock().map(|s|s.usage_trusted).unwrap_or(false){value["reason"]=json!("pricing_evidence_changed");}
                else if let Ok(actual)=Decimal::from_str(&row.total_cost_usd).map_err(|_|unavailable()).and_then(cost_micros){
                    if actual>task.budget.limit_micros{value["status"]=json!("failed");value["reason"]=json!("actual_budget_exceeded");}
                    else if !value["outputDigest"].is_null(){value["status"]=json!("completed");}
                    else {value["status"]=json!("failed");value["reason"]=json!("output_unavailable");}
                }
            } else if !attempt.has_sent(){value["status"]=json!("failed");value["reason"]=json!(attempt.state.lock().ok().and_then(|s|s.rejection.clone()).unwrap_or_else(||"admission_or_request_failed".into()));}
            else {value["reason"]=json!("usage_or_outcome_unresolved");}
            let receipt=contract::durable_result(&value);
            if contract::validate_receipt(&binding,&receipt).is_ok()&&owner.finish_execution(&s,&binding,&receipt).is_ok(){if let Ok(mut out)=output.lock(){*out=Some(value);}}
        });
        let worker = match worker {
            Ok(w) => w,
            Err(_) => {
                finished.store(true, Ordering::Release);
                workers.insert(
                    id,
                    Worker {
                        cancel,
                        thread: None,
                        finished,
                        result,
                    },
                );
                drop(workers);
                let _ = self.execution_result(b);
                return Err(unavailable());
            }
        };
        workers.insert(
            id,
            Worker {
                cancel,
                thread: Some(worker),
                finished,
                result,
            },
        );
        Ok(contract::outcome(&b, "accepted"))
    }
    fn finish_execution(&self, s: &Scope, b: &Value, result: &Value) -> Result<(), AppError> {
        let until = Instant::now() + REQUEST_BUDGET;
        loop {
            if let Some(c) = self.state.db.conn.paperclip_try_lock()? {
                let n=c.execute("UPDATE paperclip_run_bindings SET phase=?5,receipt=?6 WHERE owner=?1 AND app=?2 AND company=?3 AND run=?4 AND phase='accepted' AND binding=?7",params![self.owner,self.app.as_str(),s.company_id,s.run_id,result["status"].as_str(),result.to_string(),b.to_string()])?;
                return if n == 1 { Ok(()) } else { Err(unavailable()) };
            }
            if Instant::now() >= until {
                return Err(unavailable());
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
    pub(super) async fn request_original_response(
        &self,
        task: Task,
        route: RouteBinding,
    ) -> Result<Value, AppError> {
        if !task.valid() || self.app != AppType::Codex {
            return Err(unavailable());
        }
        self.state.db.secret_session().ensure_available()?;
        let body = json!({"model":task.model,"input":[{"role":"user","content":[{"type":"input_text","text":task.input}]}],"stream":false,"store":false,"max_output_tokens":task.max_output_tokens});
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .header("session_id", route.session_id())
            .body(axum::body::Body::from(
                serde_json::to_vec(&body).map_err(|_| unavailable())?,
            ))
            .map_err(|_| unavailable())?;
        request.extensions_mut().insert(route.clone());
        let response = self
            .state
            .proxy_service
            .paperclip_responses(request)
            .await?;
        if !self
            .state
            .proxy_service
            .request_identity_is_current("codex", Some(&route.identity))?
            || !response.status().is_success()
        {
            return Err(unavailable());
        }
        let bytes = axum::body::to_bytes(response.into_body(), 32768)
            .await
            .map_err(|_| unavailable())?;
        let response: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if response["status"] != "completed" {
            return Err(unavailable());
        }
        let mut text = String::new();
        for item in response["output"].as_array().ok_or_else(unavailable)? {
            if item["type"] != "message" || item["role"] != "assistant" {
                return Err(unavailable());
            }
            for part in item["content"].as_array().ok_or_else(unavailable)? {
                if part["type"] != "output_text" {
                    return Err(unavailable());
                }
                text.push_str(part["text"].as_str().ok_or_else(unavailable)?);
            }
        }
        if text.is_empty() || text.len() > 16384 {
            return Err(unavailable());
        }
        Ok(json!({"output":text}))
    }
}

#[cfg(test)]
mod paperclip_admission_tests {
    use super::*;
    struct TestHome {
        _temp: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }
    impl TestHome {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());
            Self {
                _temp: temp,
                previous,
            }
        }
    }
    impl Drop for TestHome {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(v) => std::env::set_var("CC_SWITCH_TEST_HOME", v),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    fn seeded() -> (TestHome, tokio::runtime::Runtime, AppState, Owner, Provider) {
        let home = TestHome::new();
        let (rt, state, owner) = super::super::tests::fixture();
        let mut p = state
            .db
            .get_provider_by_id("synthetic", "codex")
            .unwrap()
            .unwrap();
        p.website_url = Some("https://synthetic.invalid".into());
        p.meta = Some(crate::provider::ProviderMeta {
            loongport_account_id: Some(1),
            ..Default::default()
        });
        state.db.save_provider("codex", &p).unwrap();
        state.db.conn.lock().unwrap().execute("UPDATE proxy_config SET pricing_model_source='request',default_cost_multiplier='1' WHERE app_type='codex'", []).unwrap();
        crate::relay::balance::upsert_site_balance(
            &state.db,
            &(p.website_url.clone().unwrap(), 1),
            (Some(10.0), now_ms().unwrap() / 1000),
        )
        .unwrap();
        let source = json!({"openai":{"models":{"gpt-4.1":{"name":"GPT 4.1","cost":{"input":2,"output":8,"cache_read":0.5,"cache_write":2}}}}});
        rt.block_on(crate::services::models_dev::sync_pricing_for_test(
            state.db.clone(),
            &source.to_string(),
        ))
        .unwrap();
        (home, rt, state, owner, p)
    }
    fn route(state: &AppState, task: Task) -> RouteBinding {
        RouteBinding::for_test(
            state.db.clone(),
            &state.proxy_service,
            task,
            vec!["synthetic".into()],
        )
        .unwrap()
    }
    fn body() -> Value {
        json!({"model":"gpt-4.1","input":[{"role":"user","content":[{"type":"input_text","text":"synthetic request"}]}],"stream":false,"store":false,"max_output_tokens":32})
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_admission_reuses_original_price_and_balance_and_claims_once() {
        let (_home, _rt, state, _owner, p) = seeded();
        let route = route(&state, super::super::tests::task());
        route
            .before_send(&state.proxy_service, "codex", &p, &body())
            .unwrap();
        assert!(route.has_sent());
        assert!(route.evidence().unwrap().estimate_micros > 0);
        assert!(route
            .before_send(&state.proxy_service, "codex", &p, &body())
            .is_err());
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_admission_insufficient_balance_is_unsent_and_next_original_account_can_pass() {
        let (_home, _rt, state, _owner, mut p) = seeded();
        p.id = "second".into();
        p.meta.as_mut().unwrap().loongport_account_id = Some(2);
        state.db.save_provider("codex", &p).unwrap();
        crate::proxy::application_routing::set_order(
            &state.db,
            "codex",
            &["synthetic".into(), "second".into()],
        )
        .unwrap();
        crate::relay::balance::upsert_site_balance(
            &state.db,
            &(p.website_url.clone().unwrap(), 1),
            (Some(0.0), now_ms().unwrap() / 1000),
        )
        .unwrap();
        crate::relay::balance::upsert_site_balance(
            &state.db,
            &(p.website_url.clone().unwrap(), 2),
            (Some(10.0), now_ms().unwrap() / 1000),
        )
        .unwrap();
        let route = RouteBinding::for_test(
            state.db.clone(),
            &state.proxy_service,
            super::super::tests::task(),
            vec!["synthetic".into(), "second".into()],
        )
        .unwrap();
        let first = state
            .db
            .get_provider_by_id("synthetic", "codex")
            .unwrap()
            .unwrap();
        let error = route
            .before_send(&state.proxy_service, "codex", &first, &body())
            .unwrap_err();
        assert!(
            error.to_string().contains("balance_insufficient"),
            "{error}"
        );
        assert!(!route.has_sent());
        route
            .before_send(&state.proxy_service, "codex", &p, &body())
            .unwrap();
        assert_eq!(route.evidence().unwrap().provider_id, "second");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_admission_unknown_price_currency_stale_balance_and_small_budget_never_claim() {
        let (_home, _rt, state, _owner, p) = seeded();
        let mut task = super::super::tests::task();
        task.budget.currency = "CNY".into();
        let r = route(&state, task);
        assert!(r
            .before_send(&state.proxy_service, "codex", &p, &body())
            .is_err());
        assert!(!r.has_sent());
        let mut task = super::super::tests::task();
        task.budget.limit_micros = 1;
        let r = route(&state, task);
        assert!(r
            .before_send(&state.proxy_service, "codex", &p, &body())
            .is_err());
        assert!(!r.has_sent());
        crate::relay::balance::upsert_site_balance(
            &state.db,
            &(p.website_url.clone().unwrap(), 1),
            (Some(10.0), now_ms().unwrap() / 1000 - 601),
        )
        .unwrap();
        let r = route(&state, super::super::tests::task());
        assert!(r
            .before_send(&state.proxy_service, "codex", &p, &body())
            .is_err());
        assert!(!r.has_sent());
        crate::relay::balance::upsert_site_balance(
            &state.db,
            &(p.website_url.clone().unwrap(), 1),
            (Some(10.0), now_ms().unwrap() / 1000),
        )
        .unwrap();
        state.db.invalidate_pricing_observations().unwrap();
        let r = route(&state, super::super::tests::task());
        assert!(r
            .before_send(&state.proxy_service, "codex", &p, &body())
            .is_err());
        assert!(!r.has_sent());
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_admission_original_usage_row_is_confirmed_and_foreign_session_rejected() {
        let (_home, _rt, state, _owner, p) = seeded();
        let r = route(&state, super::super::tests::task());
        r.before_send(&state.proxy_service, "codex", &p, &body())
            .unwrap();
        let row = UsageLogger::new(&state.db)
            .log_with_calculation_confirmed(
                "native-test-original".into(),
                p.id.clone(),
                "codex".into(),
                "gpt-4.1".into(),
                "gpt-4.1".into(),
                "gpt-4.1".into(),
                TokenUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    ..Default::default()
                },
                Decimal::ONE,
                1,
                None,
                200,
                Some(usage_session_for_binding(&r.execution_id).unwrap()),
                None,
                false,
                10,
            )
            .unwrap();
        let mut foreign = row.clone();
        foreign.row.session_id = Some("other-run".into());
        assert!(r.record_usage(&state.proxy_service, foreign).is_err());
        r.record_usage(&state.proxy_service, row.clone()).unwrap();
        assert_eq!(r.usage().unwrap(), row.row);
        assert_eq!(
            state
                .db
                .get_request_detail_raw("native-test-original")
                .unwrap()
                .unwrap(),
            row.row
        );
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_usage_price_aba_keeps_evidence_but_rejects_completion() {
        let (_home, rt, state, _owner, p) = seeded();
        let r = route(&state, super::super::tests::task());
        r.before_send(&state.proxy_service, "codex", &p, &body())
            .unwrap();
        let admitted = r.evidence().unwrap().price_hash;
        let sync = |input, output| {
            let source = json!({"openai":{"models":{"gpt-4.1":{"name":"GPT 4.1","cost":{"input":input,"output":output,"cache_read":0.5,"cache_write":2}}}}});
            rt.block_on(crate::services::models_dev::sync_pricing_for_test(
                state.db.clone(),
                &source.to_string(),
            ))
            .unwrap();
        };
        sync(20, 80);
        let row = UsageLogger::new(&state.db)
            .log_with_calculation_confirmed(
                "native-price-aba".into(),
                p.id.clone(),
                "codex".into(),
                "gpt-4.1".into(),
                "gpt-4.1".into(),
                "gpt-4.1".into(),
                TokenUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    ..Default::default()
                },
                Decimal::ONE,
                1,
                None,
                200,
                Some(usage_session_for_binding(&r.execution_id).unwrap()),
                None,
                false,
                10,
            )
            .unwrap();
        sync(2, 8);
        assert_eq!(
            crate::services::model_pricing::observed_pricing(
                &state.db,
                "gpt-4.1",
                now_ms().unwrap()
            )
            .unwrap()
            .observation
            .row_hash,
            admitted
        );
        assert!(r.record_usage(&state.proxy_service, row.clone()).is_err());
        assert_eq!(r.usage().unwrap(), row.row);
        assert!(!r.state.lock().unwrap().usage_trusted);
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_completes_one_synthetic_send_and_reuses_saved_result() {
        original_router_case(false, "success");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_skips_unfunded_account_and_completes_next() {
        original_router_case(true, "success");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_missing_usage_never_replays() {
        original_router_case(false, "missing_usage");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_actual_overrun_keeps_usage_and_never_replays() {
        original_router_case(false, "overrun");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_logging_disabled_never_claims_completion() {
        original_router_case(false, "logging_disabled");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_usage_write_failure_never_claims_completion() {
        original_router_case(false, "write_failure");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_timeout_never_replays() {
        original_router_case(false, "timeout");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_failed_delivery_preserves_original_usage() {
        for mode in [
            "incomplete",
            "failed",
            "cancelled",
            "empty_output",
            "error_envelope",
        ] {
            original_router_case(false, mode);
        }
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_explicit_zero_usage_is_not_missing_usage() {
        original_router_case(false, "zero_usage");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_original_router_changed_balance_site_rejects_bound_run() {
        original_router_case(false, "website_changed");
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_result_waits_for_live_worker_publication() {
        let (_home, _rt, state, owner, _) = seeded();
        let t = super::super::tests::task();
        let b = owner
            .bind_execution(json!({"scope":super::super::tests::scope(&owner),"task":t}))
            .unwrap();
        let receipt = contract::durable_result(&contract::outcome(&b, "unknown"));
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE paperclip_run_bindings SET phase='unknown',receipt=?1",
                [receipt.to_string()],
            )
            .unwrap();
        let finished = Arc::new(AtomicBool::new(false));
        owner.control.executions.tasks.lock().unwrap().insert(
            b["bindingId"].as_str().unwrap().into(),
            Worker {
                cancel: Arc::new(tokio::sync::Notify::new()),
                thread: None,
                finished: finished.clone(),
                result: Arc::new(Mutex::new(None)),
            },
        );
        assert_eq!(
            owner.execution_result(b.clone()).unwrap()["status"],
            "accepted"
        );
        finished.store(true, Ordering::Release);
        assert_eq!(owner.execution_result(b).unwrap()["status"], "unknown");
    }
    fn original_router_case(skip_unfunded: bool, mode: &'static str) {
        let (_home, rt, state, owner, mut p) = seeded();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let received = count.clone();
        let (address,server)=rt.block_on(async move {
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address=listener.local_addr().unwrap();
            let app=axum::Router::new().fallback(move || {let received=received.clone(); async move {
                received.fetch_add(1,Ordering::SeqCst);
                let mut response=json!({"id":"synthetic-native-response","object":"response","status":"completed","model":"gpt-4.1","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic result"}]}],"usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}});
                if matches!(mode, "incomplete" | "failed" | "cancelled") { response["status"]=json!(mode); }
                if mode == "empty_output" { response["output"]=json!([]); }
                if mode == "error_envelope" { response["error"]=json!({"code":"synthetic_failure","message":"synthetic only"}); }
                if mode == "zero_usage" { response["usage"]=json!({"input_tokens":0,"output_tokens":0,"total_tokens":0}); }
                if mode == "missing_usage" { response.as_object_mut().unwrap().remove("usage"); }
                if mode == "overrun" { response["usage"]["output_tokens"]=json!(1_000_000); response["usage"]["total_tokens"]=json!(1_000_010); }
                if mode == "timeout" { tokio::time::sleep(Duration::from_millis(500)).await; }
                axum::Json(response)
            }});
            let server=tokio::spawn(async move {axum::serve(listener,app).await.unwrap();});
            (address,server)
        });
        p.settings_config =
            json!({"base_url":format!("http://{address}"),"api_key":"synthetic-not-a-credential"});
        state.db.save_provider("codex", &p).unwrap();
        state.db.set_current_provider("codex", &p.id).unwrap();
        if skip_unfunded {
            let origin = p.website_url.clone().unwrap();
            crate::relay::balance::upsert_site_balance(
                &state.db,
                &(origin.clone(), 1),
                (Some(0.0), now_ms().unwrap() / 1000),
            )
            .unwrap();
            p.id = "second".into();
            p.meta.as_mut().unwrap().loongport_account_id = Some(2);
            state.db.save_provider("codex", &p).unwrap();
            crate::relay::balance::upsert_site_balance(
                &state.db,
                &(origin, 2),
                (Some(10.0), now_ms().unwrap() / 1000),
            )
            .unwrap();
            crate::proxy::application_routing::set_order(
                &state.db,
                "codex",
                &["synthetic".into(), "second".into()],
            )
            .unwrap();
            state
                .db
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE proxy_config SET enabled=1 WHERE app_type='codex'",
                    [],
                )
                .unwrap();
            rt.block_on(crate::proxy::application_routing::set_failover(
                &state.db, "codex", true,
            ))
            .unwrap();
        }
        if mode == "write_failure" {
            state.db.conn.lock().unwrap().execute_batch("CREATE TRIGGER synthetic_usage_failure BEFORE INSERT ON proxy_request_logs BEGIN SELECT RAISE(FAIL, 'synthetic write failure'); END;").unwrap();
        }
        rt.block_on(
            state
                .proxy_service
                .paperclip_install_original_test_server(mode != "logging_disabled"),
        );
        let mut t = super::super::tests::task();
        t.timeout_ms = if mode == "timeout" { 100 } else { 5000 };
        let b = owner
            .bind_execution(json!({"scope":super::super::tests::scope(&owner),"task":t}))
            .unwrap();
        if mode == "website_changed" {
            p.website_url = Some("https://other.synthetic.invalid".into());
            // Update only the balance-site column, preserving the encrypted
            // settings/meta blobs so unrelated re-encryption cannot mask coverage.
            state
                .db
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE providers SET website_url=?1 WHERE id=?2 AND app_type='codex'",
                    rusqlite::params![p.website_url, p.id],
                )
                .unwrap();
            crate::relay::balance::upsert_site_balance(
                &state.db,
                &(p.website_url.clone().unwrap(), 1),
                (Some(100.0), now_ms().unwrap() / 1000),
            )
            .unwrap();
            assert!(owner
                .start_execution(json!({"binding":b,"task":t}))
                .is_err());
            assert_eq!(count.load(Ordering::SeqCst), 0);
            owner.control.executions.stop().unwrap();
            server.abort();
            return;
        }
        owner
            .start_execution(json!({"binding":b,"task":t}))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(8);
        let result = loop {
            let r = owner.execution_result(b.clone()).unwrap();
            if r["status"] != "accepted" {
                break r;
            }
            assert!(Instant::now() < until, "{r}");
            thread::sleep(Duration::from_millis(10));
        };
        if !matches!(mode, "success" | "zero_usage") {
            assert_eq!(
                result["status"],
                if matches!(
                    mode,
                    "overrun"
                        | "incomplete"
                        | "failed"
                        | "cancelled"
                        | "empty_output"
                        | "error_envelope"
                ) {
                    "failed"
                } else {
                    "unknown"
                },
                "{result}"
            );
            assert_eq!(result["providerWorkStarted"], true, "{result}");
            if matches!(
                mode,
                "overrun"
                    | "incomplete"
                    | "failed"
                    | "cancelled"
                    | "empty_output"
                    | "error_envelope"
            ) {
                assert_eq!(
                    result["reason"],
                    if mode == "overrun" {
                        "actual_budget_exceeded"
                    } else {
                        "output_unavailable"
                    }
                );
                let id = result["usage"]["requestId"].as_str().unwrap();
                assert!(state.db.get_request_detail_raw(id).unwrap().is_some());
            } else {
                assert!(result["usage"].is_null(), "{result}");
            }
            assert_eq!(
                owner
                    .start_execution(json!({"binding":b,"task":t}))
                    .unwrap(),
                result
            );
            thread::sleep(Duration::from_millis(600));
            assert_eq!(count.load(Ordering::SeqCst), 1);
            owner.control.executions.stop().unwrap();
            server.abort();
            return;
        }
        assert_eq!(result["status"], "completed", "{result}");
        assert_eq!(result["output"], "synthetic result");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let request_id = result["usage"]["requestId"].as_str().unwrap();
        let row = state
            .db
            .get_request_detail_raw(request_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.provider_id, p.id);
        if mode == "zero_usage" {
            assert_eq!(
                Decimal::from_str(&row.total_cost_usd).unwrap(),
                Decimal::ZERO
            );
        }
        assert_eq!(
            owner
                .start_execution(json!({"binding":b,"task":t}))
                .unwrap(),
            result
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM proxy_request_logs WHERE request_id=?1",
                [request_id],
            )
            .unwrap();
        assert!(owner.execution_result(b.clone()).is_err());
        assert!(owner
            .start_execution(json!({"binding":b,"task":t}))
            .is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        owner.control.executions.stop().unwrap();
        server.abort();
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_final_body_overrides_cannot_replace_input_or_add_tools() {
        let (_home, _rt, state, _owner, p) = seeded();
        for key in [
            "input",
            "tools",
            "instructions",
            "previous_response_id",
            "background",
        ] {
            let mut body = body();
            body[key] = json!("unapproved");
            let r = route(&state, super::super::tests::task());
            let error = r
                .before_send(&state.proxy_service, "codex", &p, &body)
                .unwrap_err();
            assert!(error.to_string().contains("task_body_changed"), "{error}");
            assert!(!r.has_sent());
        }
    }
}
