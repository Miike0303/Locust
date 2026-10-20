//! User-supplied policy for an exact provider ID and effective model name.
//! No model defaults, catalog, or prices are inferred here.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::Result;

pub type ModelOverrides = HashMap<String, HashMap<String, ModelPolicy>>;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedModelPolicy")]
pub struct ModelPolicy {
    pub context_window: Option<u64>,
    pub input_usd_per_million: Option<f64>,
    pub output_usd_per_million: Option<f64>,
}

#[derive(Deserialize)]
struct UncheckedModelPolicy {
    context_window: Option<u64>,
    input_usd_per_million: Option<f64>,
    output_usd_per_million: Option<f64>,
}

impl TryFrom<UncheckedModelPolicy> for ModelPolicy {
    type Error = crate::LocustError;

    fn try_from(raw: UncheckedModelPolicy) -> Result<Self> {
        let policy = Self {
            context_window: raw.context_window,
            input_usd_per_million: raw.input_usd_per_million,
            output_usd_per_million: raw.output_usd_per_million,
        };
        policy.validate()?;
        Ok(policy)
    }
}

impl ModelPolicy {
    /// Approximate input allowance, not a tokenizer or exact service-fit guarantee.
    /// Reserve one quarter (at least one token) for output/prompt space. Saturate
    /// on narrower platforms rather than truncating a user-supplied u64 window.
    pub fn approximate_input_allowance(&self) -> Option<usize> {
        self.context_window.map(|window| {
            let allowance = window.saturating_sub((window / 4).max(1));
            usize::try_from(allowance).unwrap_or(usize::MAX)
        })
    }

    /// Also validate programmatically constructed policies before using or saving.
    pub fn validate(&self) -> Result<()> {
        if self.context_window == Some(0) {
            return Err(
                anyhow::anyhow!("model policy context_window must be a positive integer").into(),
            );
        }
        match (self.input_usd_per_million, self.output_usd_per_million) {
            (None, None) => Ok(()),
            (Some(input), Some(output))
                if input.is_finite() && output.is_finite() && input >= 0.0 && output >= 0.0 =>
            {
                Ok(())
            }
            _ => Err(anyhow::anyhow!(
                "model policy prices must both be absent or both be finite nonnegative numbers"
            )
            .into()),
        }
    }
}

/// Exact, case-sensitive lookup only; callers must supply the effective model.
/// Missing, empty, or invalid in-memory policies return None (legacy behavior).
pub fn lookup_model_policy<'a>(
    overrides: &'a ModelOverrides,
    provider_id: &str,
    effective_model: &str,
) -> Option<&'a ModelPolicy> {
    let policy = overrides.get(provider_id)?.get(effective_model)?;
    (policy.validate().is_ok() && policy != &ModelPolicy::default()).then_some(policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_context_allowance_boundaries_are_safe() {
        assert_eq!(ModelPolicy::default().approximate_input_allowance(), None);
        for (window, expected) in [
            (1, 0),
            (2, 1),
            (3, 2),
            (4, 3),
            (7, 6),
            (u64::MAX, u64::MAX - u64::MAX / 4),
        ] {
            let policy = ModelPolicy {
                context_window: Some(window),
                ..Default::default()
            };
            assert_eq!(
                policy.approximate_input_allowance(),
                Some(usize::try_from(expected).unwrap_or(usize::MAX))
            );
        }
    }

    #[test]
    fn model_policy_lookup_is_exact_and_empty_or_invalid_is_absent() {
        let mut overrides = ModelOverrides::new();
        let policy = ModelPolicy {
            context_window: Some(u64::MAX),
            ..Default::default()
        };
        overrides.insert(
            "provider".into(),
            HashMap::from([("model".into(), policy.clone())]),
        );
        assert_eq!(
            lookup_model_policy(&overrides, "provider", "model"),
            Some(&policy)
        );
        for (provider, model) in [
            ("other", "model"),
            ("Provider", "model"),
            ("provider", "Model"),
            ("provider", "model "),
            ("provider", ""),
        ] {
            assert!(lookup_model_policy(&overrides, provider, model).is_none());
        }
        overrides
            .get_mut("provider")
            .unwrap()
            .insert("model".into(), ModelPolicy::default());
        assert!(lookup_model_policy(&overrides, "provider", "model").is_none());
        overrides
            .get_mut("provider")
            .unwrap()
            .get_mut("model")
            .unwrap()
            .context_window = Some(0);
        assert!(lookup_model_policy(&overrides, "provider", "model").is_none());
        let zero_prices: ModelPolicy =
            serde_json::from_str(r#"{"input_usd_per_million":0,"output_usd_per_million":0}"#)
                .unwrap();
        assert!(zero_prices.validate().is_ok());
        overrides
            .get_mut("provider")
            .unwrap()
            .insert("model".into(), zero_prices);
        assert!(lookup_model_policy(&overrides, "provider", "model").is_some());
    }
}
