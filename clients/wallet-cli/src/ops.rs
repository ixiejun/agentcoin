//! Wallet operations: signed transfers and key rotation.

use std::time::Duration;

use ac_crypto::SigAlg;
use ac_runtime::transaction::{TxParams, assemble, authorized_extensions, implicit_from, payload};
use ac_runtime::{RuntimeCall, UncheckedExtrinsic};
use anyhow::{Context, Result, bail};
use pallet_pq_accounts::{KEY_ROTATION_CONTEXT, TX_SIGNING_CONTEXT, rotation_statement};
use sp_runtime::AccountId32;
use sp_runtime::generic::Era;

use crate::client::{Inclusion, NodeClient};
use crate::wallet::Wallet;

/// How long to wait for inclusion.
pub const INCLUSION_TIMEOUT: Duration = Duration::from_secs(60);

/// An unlocked account key held in memory by a long-running service, so transactions do not
/// re-derive the key from the wallet file each time.
pub struct Signer {
    /// The account.
    pub account: AccountId32,
    /// Its current key.
    pub key: ac_crypto::sig::SigningKey,
    /// The account's first key, attached while the account is unregistered.
    pub first_key: ac_crypto::PqPublicKey,
}

impl core::fmt::Debug for Signer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Signer")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl Signer {
    /// Unlocks the wallet's current key.
    ///
    /// # Errors
    ///
    /// Wrong password.
    pub fn from_wallet(wallet: &Wallet, password: &[u8]) -> Result<Self> {
        Ok(Self {
            account: wallet.account()?,
            key: wallet.current_key(password)?,
            first_key: wallet.first_public_key(password)?,
        })
    }

    /// Builds and signs a transaction for `call`.
    ///
    /// # Errors
    ///
    /// RPC failures, or a key that does not match the registered key.
    pub async fn sign_call(
        &self,
        client: &NodeClient,
        call: RuntimeCall,
    ) -> Result<UncheckedExtrinsic> {
        let nonce = client.nonce(&self.account).await?;
        self.sign_call_at(client, call, nonce).await
    }

    /// Builds and signs a transaction for `call` with `nonce` (for several transactions in
    /// flight at once, see [`NodeClient::next_nonce`]).
    ///
    /// # Errors
    ///
    /// RPC failures, or a key that does not match the registered key.
    pub async fn sign_call_at(
        &self,
        client: &NodeClient,
        call: RuntimeCall,
        nonce: u32,
    ) -> Result<UncheckedExtrinsic> {
        let registered = client.current_key(&self.account).await?;
        let public_key = match registered {
            None => Some(self.first_key.clone()),
            Some((current, _)) => {
                if current != self.key.public_key()? {
                    bail!("the wallet's current key does not match the key registered on chain");
                }
                None
            }
        };
        let context = client.chain_context().await?;
        let params = TxParams {
            nonce,
            tip: 0,
            era: Era::Immortal,
            era_birth_hash: context.genesis_hash,
        };
        let extensions = authorized_extensions(&params);
        let digest = payload(&call, &extensions, &implicit_from(&context, &params))?;
        let mut rng = ac_crypto::OsRng::new()?;
        let signature = self.key.sign(&digest, TX_SIGNING_CONTEXT, &mut rng)?;
        Ok(assemble(
            call,
            self.account.clone(),
            signature,
            public_key,
            extensions,
        ))
    }

    /// Signs and submits `call` and waits for successful inclusion (see [`submit`]).
    ///
    /// # Errors
    ///
    /// As [`submit`].
    pub async fn submit(&self, client: &NodeClient, call: RuntimeCall) -> Result<Inclusion> {
        let xt = self.sign_call(client, call).await?;
        included(client, xt).await
    }
}

/// Builds and signs a transaction for `call` from the wallet's account. The first public key is
/// attached automatically while the account is unregistered.
///
/// # Errors
///
/// Wrong password, RPC failures, or a wallet key that does not match the registered key.
pub async fn sign_call(
    client: &NodeClient,
    wallet: &Wallet,
    password: &[u8],
    call: RuntimeCall,
) -> Result<UncheckedExtrinsic> {
    Signer::from_wallet(wallet, password)?
        .sign_call(client, call)
        .await
}

/// Signs and submits `call`, waits for inclusion and fails with the chain's error name (e.g.
/// `Providers(BelowThreshold)`) if the call did not succeed.
///
/// # Errors
///
/// See [`sign_call`] and [`NodeClient::submit_and_watch`]; a failed dispatch.
pub async fn submit(
    client: &NodeClient,
    wallet: &Wallet,
    password: &[u8],
    call: RuntimeCall,
) -> Result<Inclusion> {
    let xt = sign_call(client, wallet, password, call).await?;
    included(client, xt).await
}

async fn included(client: &NodeClient, xt: UncheckedExtrinsic) -> Result<Inclusion> {
    let inclusion = client.submit_and_watch(&xt, INCLUSION_TIMEOUT).await?;
    if !inclusion.success {
        let reason = client
            .dispatch_error(inclusion.block_hash, inclusion.index)
            .await?
            .unwrap_or_else(|| "unknown reason".to_owned());
        bail!(
            "the transaction was included in block {:?} but failed: {reason}",
            inclusion.block_hash
        );
    }
    Ok(inclusion)
}

/// Transfers `amount` smallest units to `to` and waits for inclusion.
///
/// # Errors
///
/// See [`sign_call`] and [`NodeClient::submit_and_watch`].
pub async fn transfer(
    client: &NodeClient,
    wallet: &Wallet,
    password: &[u8],
    to: &AccountId32,
    amount: u128,
) -> Result<Inclusion> {
    let call = RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death {
        dest: to.clone(),
        value: amount,
    });
    let xt = sign_call(client, wallet, password, call).await?;
    client.submit_and_watch(&xt, INCLUSION_TIMEOUT).await
}

/// Rotates the account to the next derivation index (with `alg`), waits for inclusion and, on
/// success, records the new current key in `wallet` (the caller saves the file).
///
/// # Errors
///
/// See [`sign_call`]; also fails if the rotation was included but did not succeed.
pub async fn rotate(
    client: &NodeClient,
    wallet: &mut Wallet,
    password: &[u8],
    alg: SigAlg,
) -> Result<Inclusion> {
    let who = wallet.account()?;
    let (_, index) = wallet.current();
    let next = index.checked_add(1).context("no more key indices")?;
    let new_key = wallet.key(password, alg, next)?;
    let rotations = client.current_key(&who).await?.map_or(0, |(_, r)| r);
    let genesis = client.chain_context().await?.genesis_hash;
    let statement = rotation_statement(&genesis, &who, rotations, &new_key.public_key()?);
    let mut rng = ac_crypto::OsRng::new()?;
    let proof = new_key.sign(&statement, KEY_ROTATION_CONTEXT, &mut rng)?;
    let call = RuntimeCall::PqAccounts(pallet_pq_accounts::Call::rotate_key {
        new_key: new_key.public_key()?,
        proof,
    });
    let xt = sign_call(client, wallet, password, call).await?;
    let inclusion = client.submit_and_watch(&xt, INCLUSION_TIMEOUT).await?;
    if !inclusion.success {
        bail!("rotate_key was included but failed; the wallet keeps its current key");
    }
    wallet.set_current(alg, next);
    Ok(inclusion)
}
