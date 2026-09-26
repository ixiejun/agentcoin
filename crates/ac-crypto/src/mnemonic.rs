//! 24-word BIP-39 (English) encoding of 32-byte wallet entropy.
//!
//! BIP-39 is used purely as a human-friendly, checksummed encoding of the entropy; the BIP-39
//! PBKDF2 seed and BIP-32 derivation are not used (keys come from [`crate::wallet_key_seed`]).

use alloc::string::String;

use zeroize::Zeroizing;

use crate::error::{Error, MnemonicError};
use crate::keys::{ENTROPY_LEN, WalletEntropy};

/// Number of words in an AgentCoin mnemonic (256-bit entropy).
pub const MNEMONIC_WORDS: usize = 24;

/// Encodes wallet entropy as 24 English words separated by single spaces.
///
/// # Errors
///
/// Never fails for 32-byte entropy; an encoder failure is reported as
/// [`Error::InvalidMnemonic`].
pub fn to_mnemonic(entropy: &WalletEntropy) -> Result<Zeroizing<String>, Error> {
    let mnemonic = bip39::Mnemonic::from_entropy(entropy.expose())
        .map_err(|_| Error::InvalidMnemonic(MnemonicError::WordCount))?;
    let mut out = Zeroizing::new(String::new());
    for (i, word) in mnemonic.words().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(word);
    }
    Ok(out)
}

/// Decodes a 24-word English mnemonic (any whitespace between words) back to wallet entropy.
///
/// # Errors
///
/// [`Error::InvalidMnemonic`] with [`MnemonicError::WordCount`] for anything but 24 words,
/// [`MnemonicError::UnknownWord`] for a word outside the English list and
/// [`MnemonicError::Checksum`] if the checksum does not match.
pub fn from_mnemonic(phrase: &str) -> Result<WalletEntropy, Error> {
    if phrase.split_whitespace().count() != MNEMONIC_WORDS {
        return Err(Error::InvalidMnemonic(MnemonicError::WordCount));
    }
    let mnemonic =
        bip39::Mnemonic::parse_in_normalized(bip39::Language::English, phrase).map_err(|e| {
            Error::InvalidMnemonic(match e {
                bip39::Error::UnknownWord(_) => MnemonicError::UnknownWord,
                bip39::Error::InvalidChecksum => MnemonicError::Checksum,
                _ => MnemonicError::WordCount,
            })
        })?;
    let (array, len) = mnemonic.to_entropy_array();
    let array = Zeroizing::new(array);
    let bytes: [u8; ENTROPY_LEN] = array
        .get(..len)
        .and_then(|s| s.try_into().ok())
        .ok_or(Error::InvalidMnemonic(MnemonicError::WordCount))?;
    Ok(WalletEntropy::new(bytes))
}
