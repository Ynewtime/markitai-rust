//! Recorded work belongs to an attempt, independently of retained output files.
use markitai_core::ConversionUsage;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Operation {
    Convert,
    Retry,
    Enhance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Done,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Attempt {
    pub(crate) operation: Operation,
    pub(crate) status: Status,
    pub(crate) error: Option<String>,
    pub(crate) usage: ConversionUsage,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AttemptDiagnostics {
    pub(crate) last_attempt: Attempt,
}

pub(crate) fn observed(usage: &ConversionUsage) -> bool {
    usage.requests > 0
        || usage.input_tokens > 0
        || usage.output_tokens > 0
        || !usage.by_model.is_empty()
}

impl AttemptDiagnostics {
    pub(crate) fn completed(operation: Operation, usage: ConversionUsage) -> Option<Self> {
        observed(&usage).then_some(Self {
            last_attempt: Attempt {
                operation,
                status: Status::Done,
                error: None,
                usage,
            },
        })
    }

    pub(crate) fn failed(
        operation: Operation,
        error: impl Into<String>,
        usage: ConversionUsage,
    ) -> Option<Self> {
        observed(&usage).then(|| Self {
            last_attempt: Attempt {
                operation,
                status: Status::Error,
                error: Some(error.into()),
                usage,
            },
        })
    }

    /// Validate persisted native observations before they enter recovery state.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let attempt = &self.last_attempt;
        let usage = &attempt.usage;
        let invalid = || "Invalid native attempt diagnostics".to_owned();
        if !observed(usage)
            || !usage.cost_usd.is_finite()
            || usage.cost_usd < 0.0
            || (attempt.status == Status::Done) != attempt.error.is_none()
        {
            return Err(invalid());
        }
        for model in usage.by_model.values() {
            if !model.is_object()
                || ["requests", "input_tokens", "output_tokens"]
                    .iter()
                    .any(|field| model[*field].as_u64().is_none())
                || !model["cost_usd"]
                    .as_f64()
                    .is_some_and(|cost| cost.is_finite() && cost >= 0.0)
            {
                return Err(invalid());
            }
        }
        if !usage.by_model.is_empty() {
            for (field, total) in [
                ("requests", usage.requests),
                ("input_tokens", usage.input_tokens),
                ("output_tokens", usage.output_tokens),
            ] {
                let recorded = usage.by_model.values().fold(0u64, |sum, model| {
                    sum.saturating_add(model[field].as_u64().unwrap_or(0))
                });
                if recorded != total {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self, String> {
        let diagnostics: Self = serde_json::from_value(value.clone())
            .map_err(|_| "Invalid native attempt diagnostics".to_owned())?;
        diagnostics.validate()?;
        Ok(diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paid_zero() -> ConversionUsage {
        ConversionUsage {
            requests: 1,
            by_model: serde_json::from_value(json!({"fixture":{
                "requests":1,"input_tokens":0,"output_tokens":0,"cost_usd":0.0
            }}))
            .unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn known_zero_is_distinct_from_missing_and_preserves_the_attempt() {
        assert!(
            AttemptDiagnostics::completed(Operation::Convert, ConversionUsage::default()).is_none()
        );
        assert!(
            AttemptDiagnostics::failed(
                Operation::Retry,
                "before model",
                ConversionUsage::default()
            )
            .is_none()
        );
        let value =
            AttemptDiagnostics::failed(Operation::Enhance, "original error", paid_zero()).unwrap();
        let encoded = serde_json::to_value(value).unwrap();
        let restored = AttemptDiagnostics::from_value(&encoded).unwrap();
        assert_eq!(restored.last_attempt.operation, Operation::Enhance);
        assert_eq!(restored.last_attempt.status, Status::Error);
        assert_eq!(
            restored.last_attempt.error.as_deref(),
            Some("original error")
        );
        assert_eq!(restored.last_attempt.usage.requests, 1);
    }

    #[test]
    fn persisted_negative_fractional_or_inconsistent_observations_are_rejected() {
        let value = serde_json::to_value(
            AttemptDiagnostics::completed(Operation::Convert, paid_zero()).unwrap(),
        )
        .unwrap();
        for (pointer, replacement) in [
            ("/last_attempt/usage/requests", json!(-1)),
            ("/last_attempt/usage/requests", json!(2)),
            ("/last_attempt/usage/input_tokens", json!(1.5)),
            ("/last_attempt/usage/cost_usd", json!(-0.1)),
            (
                "/last_attempt/usage/by_model/fixture/input_tokens",
                json!("12"),
            ),
            ("/last_attempt/usage/by_model/fixture/cost_usd", json!(-1)),
            ("/last_attempt/status", json!("error")),
            ("/last_attempt/operation", json!("unknown")),
        ] {
            let mut malformed = value.clone();
            *malformed.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                AttemptDiagnostics::from_value(&malformed).is_err(),
                "{pointer}"
            );
        }
    }
}
