//! 被动模型监控的入口与判定（M1）。
//!
//! 响应流上的观察器产出 [`EvidenceBatch`]，经 [`PassiveIngress`]（有界 mpsc，
//! 满即丢、永不阻塞转发路径）交给消费侧；[`evaluate_passive`] 把单次请求的
//! 证据降为一份报告——**只报异常不背书**：高置信异源指纹直接 Anomaly，
//! 次要信号 Suspicious，干净流量不产报告，任何输入都不返回 Trusted。

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::relay::model_verification::types::{
    EvidenceCode, EvidenceFact, EvidenceLevel, EvidenceOutcome, TargetKey, Verdict, RULES_VERSION,
};

pub const PASSIVE_INGRESS_CAPACITY: usize = 128;
/// 单个在途 SSE 事件的最大观察字节数；超限即放弃对该请求的进一步观察（有界内存）。
pub const MAX_SSE_EVENT_BYTES: usize = 256 * 1024;
/// 非流式响应的最大观察字节数；超限记 oversized 不再解析。
pub const MAX_RESPONSE_INSPECTION_BYTES: usize = 2 * 1024 * 1024;
/// 自报身份短语匹配保留的尾部窗口字节数。
pub const SELF_ID_TAIL_BYTES: usize = 256;

/// 一次请求的观察结果。facts 已消毒（只含 code+outcome），可安全落库。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceBatch {
    pub target: TargetKey,
    pub completed: bool,
    pub facts: Vec<EvidenceFact>,
    pub observed_at: i64,
}

#[derive(Clone)]
pub struct PassiveIngress {
    sender: mpsc::Sender<EvidenceBatch>,
}

impl PassiveIngress {
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<EvidenceBatch>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (Self { sender }, receiver)
    }

    pub fn try_submit(&self, batch: EvidenceBatch) -> bool {
        self.sender.try_send(batch).is_ok()
    }
}

/// 把单次请求的被动证据降为一份报告。
///
/// 分级（spec §2）：异源指纹 = 高置信换芯证据 → Anomaly；模型名不符（sub2api
/// 会回写 model 字段，失败仅说明连回写都没做的廉价转发）、thinking 签名缺失
/// → Suspicious；其余（含全 Passed/Skipped）不产报告。**任何输入都不返回
/// Trusted**——被动观察是「顺带看到什么算什么」，干净流量不构成背书。
pub fn evaluate_passive(
    batch: &EvidenceBatch,
) -> Option<crate::relay::model_verification::types::VerificationReport> {
    use crate::relay::model_verification::types::VerificationReport;

    let failed = |code: EvidenceCode| {
        batch
            .facts
            .iter()
            .any(|fact| fact.code == code && fact.outcome == EvidenceOutcome::Failed)
    };

    let verdict = if failed(EvidenceCode::ForeignProtocol)
        || failed(EvidenceCode::ForeignSelfIdentification)
    {
        Verdict::Anomaly
    } else if failed(EvidenceCode::ModelMatch)
        || failed(EvidenceCode::ThinkingSignature)
        || failed(EvidenceCode::SignatureContinuation)
    {
        Verdict::Suspicious
    } else {
        return None;
    };

    Some(VerificationReport {
        target: batch.target.clone(),
        verdict,
        evidence_level: EvidenceLevel::ProtocolBehavior,
        facts: batch.facts.clone(),
        rules_version: RULES_VERSION,
        checked_at: batch.observed_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(facts: &[(EvidenceCode, EvidenceOutcome)]) -> EvidenceBatch {
        EvidenceBatch {
            target: TargetKey::new("loongport-0123456789abcdef", "codex", "gpt-5.6"),
            completed: true,
            facts: facts
                .iter()
                .map(|(code, outcome)| EvidenceFact {
                    code: *code,
                    outcome: *outcome,
                })
                .collect(),
            observed_at: 1_700_000_000,
        }
    }

    #[test]
    fn foreign_fingerprints_are_anomaly() {
        for code in [
            EvidenceCode::ForeignProtocol,
            EvidenceCode::ForeignSelfIdentification,
        ] {
            let report = evaluate_passive(&batch(&[(code, EvidenceOutcome::Failed)]))
                .unwrap_or_else(|| {
                    panic!("{code:?} Failed 必须产出报告");
                });
            assert_eq!(report.verdict, Verdict::Anomaly, "{code:?}");
            assert_eq!(report.evidence_level, EvidenceLevel::ProtocolBehavior);
            assert_eq!(report.target.model, "gpt-5.6");
            assert_eq!(report.checked_at, 1_700_000_000);
        }
    }

    #[test]
    fn secondary_failures_are_suspicious_not_anomaly() {
        for code in [
            EvidenceCode::ModelMatch,
            EvidenceCode::ThinkingSignature,
            EvidenceCode::SignatureContinuation,
        ] {
            let report = evaluate_passive(&batch(&[(code, EvidenceOutcome::Failed)]))
                .unwrap_or_else(|| panic!("{code:?} Failed 必须产出报告"));
            assert_eq!(report.verdict, Verdict::Suspicious, "{code:?}");
        }
    }

    #[test]
    fn clean_traffic_produces_no_report() {
        let all_passed: Vec<_> = EvidenceCode::ALL
            .into_iter()
            .map(|code| (code, EvidenceOutcome::Passed))
            .collect();
        assert!(evaluate_passive(&batch(&all_passed)).is_none());
    }

    #[test]
    fn skipped_only_produces_no_report() {
        let all_skipped: Vec<_> = EvidenceCode::ALL
            .into_iter()
            .map(|code| (code, EvidenceOutcome::Skipped))
            .collect();
        assert!(evaluate_passive(&batch(&all_skipped)).is_none());
    }

    #[test]
    fn passive_never_returns_trusted() {
        for code in EvidenceCode::ALL {
            for outcome in [
                EvidenceOutcome::Passed,
                EvidenceOutcome::Failed,
                EvidenceOutcome::Skipped,
            ] {
                if let Some(report) = evaluate_passive(&batch(&[(code, outcome)])) {
                    assert_ne!(report.verdict, Verdict::Trusted, "{code:?} {outcome:?}");
                }
            }
        }
    }

    #[tokio::test]
    async fn try_submit_never_blocks_or_panics() {
        let (ingress, mut receiver) = PassiveIngress::channel(1);
        assert!(ingress.try_submit(batch(&[])));
        assert!(!ingress.try_submit(batch(&[])), "满即丢，不阻塞不 panic");
        assert!(receiver.try_recv().is_ok(), "第一条必须已入队");
    }
}
