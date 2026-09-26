//! X-Wing (draft-connolly-cfrg-xwing-kem-06) backend. The only place that touches `x-wing`.

use alloc::vec::Vec;

use x_wing::{Decapsulate, Decapsulator, KeyExport};

use crate::error::Error;

#[cfg(any(feature = "rand", feature = "deterministic"))]
use super::ENCAPSULATION_RANDOMNESS_LEN;
use super::SHARED_SECRET_LEN;

/// X-Wing decapsulation key (zeroized on drop by the backend).
pub(crate) struct XWingSecret(x_wing::DecapsulationKey);

impl XWingSecret {
    pub(crate) fn from_seed(seed: &[u8; 32]) -> Self {
        Self(x_wing::DecapsulationKey::from(*seed))
    }

    pub(crate) fn public_key(&self) -> Vec<u8> {
        self.0.encapsulation_key().to_bytes().as_slice().to_vec()
    }

    pub(crate) fn decapsulate(&self, ciphertext: &[u8]) -> Result<[u8; SHARED_SECRET_LEN], Error> {
        let ss = self
            .0
            .decapsulate_slice(ciphertext)
            .map_err(|_| Error::InvalidLength {
                expected: x_wing::CIPHERTEXT_SIZE,
                actual: ciphertext.len(),
            })?;
        Ok(ss.into())
    }
}

#[cfg(any(feature = "rand", feature = "deterministic"))]
pub(crate) fn encapsulate(
    public_key: &[u8],
    randomness: &[u8; ENCAPSULATION_RANDOMNESS_LEN],
) -> Result<(Vec<u8>, [u8; SHARED_SECRET_LEN]), Error> {
    let ek = x_wing::EncapsulationKey::try_from(public_key).map_err(|_| Error::InvalidKey)?;
    let (ct, ss) = ek.encapsulate_deterministic(&(*randomness).into());
    Ok((ct.as_slice().to_vec(), ss.into()))
}
