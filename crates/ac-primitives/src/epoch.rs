//! Epochs: fixed-length runs of blocks (plan §4.2, design D6 of `m2-finality`).
//!
//! Block `b ≥ 1` belongs to epoch `⌊(b − 1) / L⌋`; the first block of an epoch is its
//! boundary block, where the authority set may change. Shared by the runtime and the node so
//! both compute epochs identically.

/// Epoch index.
pub type EpochIndex = u64;

/// Why an epoch length is not allowed.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochLengthError {
    /// Shorter than twice the number of authorities, so some authority could get fewer than
    /// two slots per epoch (it needs one to commit and one to reveal randomness).
    TooShort {
        /// Smallest allowed length.
        minimum: u64,
    },
}

impl core::fmt::Display for EpochLengthError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort { minimum } => write!(f, "epoch length must be at least {minimum}"),
        }
    }
}

/// Checks that `length` is at least twice `authorities`.
///
/// # Errors
///
/// [`EpochLengthError::TooShort`] otherwise.
pub fn check_epoch_length(length: u64, authorities: u64) -> Result<(), EpochLengthError> {
    // Saturating: an absurd authority count must fail the check, not wrap to a small minimum.
    let minimum = authorities.saturating_mul(2).max(1);
    if length < minimum {
        return Err(EpochLengthError::TooShort { minimum });
    }
    Ok(())
}

/// The epoch of block `number`. Block 0 (genesis) is counted as epoch 0; `None` if `length`
/// is zero.
#[must_use]
pub fn epoch_of(number: u64, length: u64) -> Option<EpochIndex> {
    number.saturating_sub(1).checked_div(length)
}

/// Whether block `number` is the first block of its epoch. Genesis (block 0) is not a
/// boundary: nothing executes there.
#[must_use]
pub fn is_boundary(number: u64, length: u64) -> bool {
    number >= 1 && number.saturating_sub(1).checked_rem(length) == Some(0)
}

/// The boundary block that starts `epoch`; `None` on overflow.
#[must_use]
pub fn epoch_start(epoch: EpochIndex, length: u64) -> Option<u64> {
    epoch.checked_mul(length)?.checked_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // consensus/validator-set Scenario "纪元编号".
    #[test]
    fn epoch_numbering() {
        assert_eq!(epoch_of(1, 20), Some(0));
        assert_eq!(epoch_of(20, 20), Some(0));
        assert_eq!(epoch_of(21, 20), Some(1));
        assert_eq!(epoch_of(0, 20), Some(0));
        assert_eq!(epoch_of(5, 0), None);
        assert!(is_boundary(1, 20));
        assert!(!is_boundary(20, 20));
        assert!(is_boundary(21, 20));
        assert!(!is_boundary(0, 20));
        assert!(!is_boundary(5, 0));
        assert_eq!(epoch_start(1, 20), Some(21));
        assert_eq!(epoch_start(u64::MAX, 20), None);
    }

    // consensus/validator-set Scenario "纪元过短".
    #[test]
    fn epoch_length_minimum() {
        assert_eq!(
            check_epoch_length(7, 4),
            Err(EpochLengthError::TooShort { minimum: 8 })
        );
        assert_eq!(check_epoch_length(8, 4), Ok(()));
        assert_eq!(check_epoch_length(10, 1), Ok(()));
        assert!(check_epoch_length(u64::MAX - 1, u64::MAX).is_err());
    }

    proptest::proptest! {
        #[test]
        fn boundaries_start_epochs(number in 1u64..1_000_000, length in 1u64..10_000) {
            let epoch = epoch_of(number, length).unwrap();
            proptest::prop_assert_eq!(is_boundary(number, length), epoch_start(epoch, length) == Some(number));
        }
    }
}
