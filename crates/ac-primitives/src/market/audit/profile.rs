//! Per-model audit profiles (OpenSpec change m6-audit-hardening design D1; spec `market/audit`
//! "按模型的审计档案").
//!
//! Re-check thresholds and the statistical judgment's parameters are calibrated per model, and a
//! model ID already fixes the precision. A model is audited only under a profile naming both
//! versions: its own, set by the administration once its calibration report is in, or, for a
//! bf16 model without one, the chain's default (none on live chains). [`resolve`] gives a model's
//! profile; a model without one is *uncalibrated*: verdicts on it are refused and its receipts
//! earn no market work (fail-closed).

use parity_scale_codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

use super::stats::stats_params;
use crate::market::model::QuantType;

/// Re-check thresholds versions the runtime knows, oldest first: the versions of
/// `ac-market-proto`'s `AUDIT_THRESHOLDS` that auditors still implement. `ac-market-proto`
/// checks that its current version is listed and is the newest.
pub const THRESHOLDS_VERSIONS: &[u16] = &[4];

/// The newest of [`THRESHOLDS_VERSIONS`]: test chains audit under it by default.
pub const CURRENT_THRESHOLDS_VERSION: u16 = 4;

/// Whether `version` is a re-check thresholds version the runtime knows.
#[must_use]
pub fn thresholds_known(version: u16) -> bool {
    THRESHOLDS_VERSIONS.contains(&version)
}

/// The versions a model is audited under.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct AuditProfile {
    /// Re-check thresholds version verdicts on the model must use.
    pub thresholds_version: u16,
    /// Statistical judgment parameter version verdicts on the model must use.
    pub stats_version: u16,
}

/// Where a model's profile comes from.
///
/// Wire-format enum: variant indices are explicit and never reused.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub enum ProfileSource {
    /// Set for the model itself.
    #[codec(index = 0)]
    Own,
    /// The chain's default for bf16 models without a profile of their own.
    #[codec(index = 1)]
    Default,
}

/// Why a profile cannot be set.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// Only bf16 models are re-checked.
    NotBf16,
    /// The thresholds version is not one the runtime knows.
    UnknownThresholds,
    /// The statistical parameter version is not one the runtime knows.
    UnknownStats,
    /// A version would decrease.
    Decrease,
}

impl core::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NotBf16 => "only bf16 models are audited",
            Self::UnknownThresholds => "the thresholds version is not built into the runtime",
            Self::UnknownStats => "the statistical parameter version is not built into the runtime",
            Self::Decrease => "profile versions may only increase",
        })
    }
}

impl AuditProfile {
    /// Checks both versions are built into the runtime.
    ///
    /// # Errors
    ///
    /// [`ProfileError::UnknownThresholds`] or [`ProfileError::UnknownStats`].
    pub fn check_known(&self) -> Result<(), ProfileError> {
        if !thresholds_known(self.thresholds_version) {
            return Err(ProfileError::UnknownThresholds);
        }
        if stats_params(self.stats_version).is_none() {
            return Err(ProfileError::UnknownStats);
        }
        Ok(())
    }

    /// Checks `self` may become the profile of a model quantized as `quant` whose profile is
    /// now `current` (its own or the default, as [`resolve`] gives it): a bf16 model, known
    /// versions, neither lower than now.
    ///
    /// # Errors
    ///
    /// The first rule broken.
    pub fn check_update(
        &self,
        quant: QuantType,
        current: Option<&AuditProfile>,
    ) -> Result<(), ProfileError> {
        if quant != QuantType::Bf16 {
            return Err(ProfileError::NotBf16);
        }
        self.check_known()?;
        match current {
            Some(c)
                if self.thresholds_version < c.thresholds_version
                    || self.stats_version < c.stats_version =>
            {
                Err(ProfileError::Decrease)
            }
            _ => Ok(()),
        }
    }
}

/// The profile of a model quantized as `quant` with profile `own` (if set) under the chain's
/// `default`: its own, else the default for a bf16 model, else none (uncalibrated).
#[must_use]
pub fn resolve(
    own: Option<AuditProfile>,
    default: Option<AuditProfile>,
    quant: QuantType,
) -> Option<(AuditProfile, ProfileSource)> {
    if let Some(p) = own {
        return Some((p, ProfileSource::Own));
    }
    match (quant, default) {
        (QuantType::Bf16, Some(d)) => Some((d, ProfileSource::Default)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::audit::stats::CURRENT_STATS;

    const P: AuditProfile = AuditProfile {
        thresholds_version: 4,
        stats_version: 1,
    };

    #[test]
    fn current_versions_are_known() {
        assert_eq!(
            THRESHOLDS_VERSIONS.iter().max(),
            Some(&CURRENT_THRESHOLDS_VERSION)
        );
        assert!(thresholds_known(4));
        assert!(!thresholds_known(3));
        assert_eq!(
            AuditProfile {
                thresholds_version: 4,
                stats_version: CURRENT_STATS.version,
            }
            .check_known(),
            Ok(())
        );
    }

    // Spec "按模型的审计档案", scenario "设定档案".
    #[test]
    fn first_profile_of_a_bf16_model() {
        assert_eq!(P.check_update(QuantType::Bf16, None), Ok(()));
    }

    // Scenario "非 BF16 模型不能设定档案".
    #[test]
    fn non_bf16_models_get_no_profile() {
        assert_eq!(
            P.check_update(QuantType::Int8, None),
            Err(ProfileError::NotBf16)
        );
    }

    // Spec "参数与护栏", scenarios "阈值版本不能回退" and "未内置的统计参数版本".
    #[test]
    fn versions_only_increase_and_must_be_built_in() {
        let lower = AuditProfile {
            thresholds_version: 3,
            ..P
        };
        assert_eq!(
            lower.check_update(QuantType::Bf16, Some(&P)),
            Err(ProfileError::UnknownThresholds)
        );
        let unknown = AuditProfile {
            stats_version: 99,
            ..P
        };
        assert_eq!(
            unknown.check_update(QuantType::Bf16, Some(&P)),
            Err(ProfileError::UnknownStats)
        );
        let newer_current = AuditProfile {
            stats_version: 2,
            ..P
        };
        assert_eq!(
            P.check_update(QuantType::Bf16, Some(&newer_current)),
            Err(ProfileError::Decrease)
        );
        // Setting the same versions again is not a decrease.
        assert_eq!(P.check_update(QuantType::Bf16, Some(&P)), Ok(()));
    }

    // Scenario "默认档案"; scenario "未校准模型的裁决被拒绝" rests on the `None`.
    #[test]
    fn resolution_order() {
        let other = AuditProfile {
            stats_version: 2,
            ..P
        };
        assert_eq!(
            resolve(Some(other), Some(P), QuantType::Bf16),
            Some((other, ProfileSource::Own))
        );
        assert_eq!(
            resolve(None, Some(P), QuantType::Bf16),
            Some((P, ProfileSource::Default))
        );
        assert_eq!(resolve(None, None, QuantType::Bf16), None);
        assert_eq!(resolve(None, Some(P), QuantType::Int8), None);
    }

    #[test]
    fn encodings_are_fixed() {
        assert_eq!(P.encode(), [4, 0, 1, 0]);
        assert_eq!(ProfileSource::Default.encode(), [1]);
    }
}
