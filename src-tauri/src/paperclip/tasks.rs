//! Small LoongPort task facade over the existing execution owner, not a scheduler.
use super::contract::Task;
use super::*;

impl Owner {
    pub(crate) fn create_local_task(
        &self,
        request_id: String,
        input: String,
        model: String,
    ) -> Result<Value, AppError> {
        if !id(&request_id) || request_id.len() > 100 {
            return Err(unavailable());
        }
        let _gate = self
            .control
            .local_task_gate
            .try_lock()
            .map_err(|_| unavailable())?;
        let tasks = [
            ("analysis", "独立分析以下任务"),
            ("review", "独立检查以下任务的风险和遗漏"),
        ]
        .map(|(agent, instruction)| {
            (
                agent,
                Task {
                    version: 3,
                    input: format!("{instruction}：\n{input}"),
                    model: model.clone(),
                    max_output_tokens: 1024,
                    timeout_ms: 30_000,
                    budget: None,
                },
            )
        });
        if input.trim().is_empty() || tasks.iter().any(|(_, task)| !task.valid()) {
            return Err(unavailable());
        }
        let existing: Vec<_> = self
            .local_bindings()?
            .into_iter()
            .filter(|(id, _, _, _)| id == &request_id)
            .collect();
        if !existing.is_empty() {
            // An uncertain retry reads the original claim, including its old
            // revision. It must never fill in or replay a partially started task.
            for (_, agent, binding, _) in existing {
                let task = &tasks
                    .iter()
                    .find(|(id, _)| *id == agent)
                    .ok_or_else(unavailable)?
                    .1;
                let scope = self.execution_scope(&binding)?;
                contract::task_for_start(&binding, &scope, self.app.as_str(), task.clone())?;
            }
            return self.local_tasks();
        }
        let snapshot = self.snapshot_until(Instant::now() + REQUEST_BUDGET)?;
        let revision = snapshot["revision"].as_str().ok_or_else(unavailable)?;
        let mut pending = Vec::new();
        for (agent, task) in tasks {
            let scope = Scope {
                company_id: "loongport".into(),
                task_id: request_id.clone(),
                agent_id: agent.into(),
                run_id: format!("{request_id}-{agent}"),
                resource_id: snapshot::resource_id(&self.owner, self.app.as_str()),
                owner_id: self.owner.clone(),
                snapshot_revision: revision.into(),
            };
            let binding = self.bind_execution_until(
                json!({"scope":scope,"task":task}),
                Instant::now() + REQUEST_BUDGET,
            )?;
            pending.push((binding, task));
        }
        for (binding, task) in pending {
            // The original durable claim makes repeated submission non-replaying.
            self.start_execution_until(
                json!({"binding":binding,"task":task}),
                Instant::now() + REQUEST_BUDGET,
            )?;
        }
        self.local_tasks()
    }
    fn local_bindings(&self) -> Result<Vec<(String, String, Value, String)>, AppError> {
        let c = self.db_admission(Instant::now() + REQUEST_BUDGET)?;
        let mut q = c.prepare("SELECT scope,binding,phase FROM paperclip_run_bindings WHERE owner=?1 AND app=?2 AND company='loongport' ORDER BY rowid DESC").map_err(|_|unavailable())?;
        let rows = q
            .query_map(params![self.owner, self.app.as_str()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|_| unavailable())?;
        let mut result = Vec::new();
        for row in rows {
            let (scope, binding, phase) = row.map_err(|_| unavailable())?;
            let s: Scope = serde_json::from_str(&scope).map_err(|_| unavailable())?;
            let b: Value = serde_json::from_str(&binding).map_err(|_| unavailable())?;
            self.execution_scope(&b)?;
            result.push((s.task_id, s.agent_id, b, phase));
        }
        Ok(result)
    }
    pub(crate) fn local_tasks(&self) -> Result<Value, AppError> {
        let mut result = Vec::new();
        for (task, agent, b, phase) in self.local_bindings()? {
            let value = if phase == "bound" {
                contract::outcome(&b, "not_started")
            } else {
                self.execution_result(b)?
            };
            result.push(json!({"taskId":task,"agentId":agent,"result":value}));
        }
        Ok(json!(result))
    }
    pub(crate) fn cancel_local_task(&self, task_id: &str) -> Result<Value, AppError> {
        if !id(task_id) {
            return Err(unavailable());
        }
        let _gate = self
            .control
            .local_task_gate
            .try_lock()
            .map_err(|_| unavailable())?;
        let mut found = false;
        for (task, _, b, phase) in self.local_bindings()? {
            if task == task_id {
                found = true;
                if phase == "bound" {
                    let scope = self.execution_scope(&b)?;
                    let mut result = contract::outcome(&b, "failed");
                    result["providerWorkStarted"] = json!(false);
                    result["reason"] = json!("cancelled_before_start");
                    let receipt = contract::durable_result(&result);
                    contract::validate_receipt(&b, &receipt)?;
                    let c = self.db_admission(Instant::now() + REQUEST_BUDGET)?;
                    c.execute("UPDATE paperclip_run_bindings SET phase='failed',receipt=?1 WHERE owner=?2 AND app=?3 AND company='loongport' AND run=?4 AND binding=?5 AND phase='bound'", params![receipt.to_string(),self.owner,self.app.as_str(),scope.run_id,b.to_string()]).map_err(|_|unavailable())?;
                } else {
                    self.cancel_execution_until(b, Instant::now() + REQUEST_BUDGET)?;
                }
            }
        }
        if !found {
            return Err(unavailable());
        }
        // Cancellation is a request, never a claim that the remote provider stopped.
        self.local_tasks()
    }
    pub(crate) fn stop_local_tasks(&self) -> Result<(), AppError> {
        self.control.stop.store(true, Ordering::Release);
        self.control.executions.stop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bound(owner: &Owner) -> (Value, Task) {
        let mut scope = super::super::tests::scope(owner);
        scope["companyId"] = json!("loongport");
        let mut task = super::super::tests::task();
        task.budget = None;
        let b = owner
            .bind_execution(json!({"scope":scope,"task":task}))
            .unwrap();
        (b, task)
    }
    #[test]
    #[serial_test::serial]
    fn local_task_failure_is_visible_and_duplicate_start_never_replays() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let (_rt, _state, owner) = super::super::tests::fixture();
        let (b, task) = bound(&owner);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        owner
            .start_execution_with_until(
                json!({"binding":b,"task":task}),
                move |_, _| async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err(unavailable())
                },
                Instant::now() + REQUEST_BUDGET,
            )
            .unwrap();
        owner
            .start_execution_with_until(
                json!({"binding":b,"task":task}),
                |_, _| async { panic!("duplicate runner") },
                Instant::now() + REQUEST_BUDGET,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while owner.execution_result(b.clone()).unwrap()["status"] == "accepted"
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            owner.local_tasks().unwrap()[0]["result"]["status"],
            "failed"
        );
        owner.stop_local_tasks().unwrap();
    }
    #[test]
    #[serial_test::serial]
    fn local_task_cancel_is_repeatable_and_does_not_claim_remote_stop() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let (_rt, _state, owner) = super::super::tests::fixture();
        let (b, task) = bound(&owner);
        owner
            .start_execution_with_until(
                json!({"binding":b,"task":task}),
                |_, _| std::future::pending::<Result<Value, AppError>>(),
                Instant::now() + REQUEST_BUDGET,
            )
            .unwrap();
        assert!(owner.cancel_local_task("foreign-task").is_err());
        owner.cancel_local_task("task-a").unwrap();
        owner.cancel_local_task("task-a").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let value = loop {
            let result = owner.execution_result(b.clone()).unwrap();
            if result["status"] != "accepted" {
                break result;
            }
            assert!(Instant::now() < deadline, "cancellation did not finish");
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(value["providerStopped"], false);
        owner.stop_local_tasks().unwrap();
    }
    #[test]
    #[serial_test::serial]
    fn partial_start_cancel_and_revision_change_never_restart_existing_request() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let (_rt, state, owner) = super::super::tests::fixture();
        let mut bindings = Vec::new();
        for (agent, instruction) in [
            ("analysis", "独立分析以下任务"),
            ("review", "独立检查以下任务的风险和遗漏"),
        ] {
            let mut scope = super::super::tests::scope(&owner);
            scope["companyId"] = json!("loongport");
            scope["taskId"] = json!("partial");
            scope["agentId"] = json!(agent);
            scope["runId"] = json!(format!("partial-{agent}"));
            let task = Task {
                version: 3,
                input: format!("{instruction}：\nReview"),
                model: "test-model".into(),
                max_output_tokens: 1024,
                timeout_ms: 30_000,
                budget: None,
            };
            let b = owner
                .bind_execution(json!({"scope":scope,"task":task}))
                .unwrap();
            bindings.push((b, task));
        }
        let (first, first_task) = bindings[0].clone();
        owner
            .start_execution_with_until(
                json!({"binding":first,"task":first_task}),
                |_, _| std::future::pending::<Result<Value, AppError>>(),
                Instant::now() + REQUEST_BUDGET,
            )
            .unwrap();
        let (second, second_task) = bindings[1].clone();
        assert!(owner
            .start_execution_with_until(
                json!({"binding":second,"task":second_task}),
                |_, _| async { panic!("expired second start") },
                Instant::now()
            )
            .is_err());
        owner.cancel_local_task("partial").unwrap();
        assert_eq!(
            owner.execution_result(second.clone()).unwrap()["reason"],
            "cancelled_before_start"
        );
        state.db.conn.lock().unwrap().execute("INSERT OR REPLACE INTO settings(key,value) VALUES('application_priority_codex','[]')",[]).unwrap();
        let rows = owner
            .create_local_task("partial".into(), "Review".into(), "test-model".into())
            .unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(
            owner.execution_result(second).unwrap()["reason"],
            "cancelled_before_start"
        );
        assert!(owner
            .create_local_task("partial".into(), "Changed".into(), "test-model".into())
            .is_err());
        owner.stop_local_tasks().unwrap();
    }
    #[test]
    #[serial_test::serial]
    fn invalid_task_does_not_create_runs() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let (_rt, _state, owner) = super::super::tests::fixture();
        assert!(owner
            .create_local_task("bad/id".into(), "test".into(), "test".into())
            .is_err());
        assert!(owner
            .create_local_task("valid".into(), "".into(), "test".into())
            .is_err());
        assert_eq!(owner.local_tasks().unwrap(), json!([]));
    }
}
