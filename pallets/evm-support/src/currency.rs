//! [`ReviveCurrency`]: the currency `pallet-revive` sees, which never creates issuance.
//!
//! `pallet-revive` mints an existential deposit into every new contract account
//! (`Mutate::mint_into`) and emulates fee deposits in dry runs with `Balanced::issue`. Both
//! would create ATC outside the emission rules (D9), and the node's issuance check rejects a
//! block that mints outside emission settlement (spec `node/invariants`). This adapter forwards
//! every fungible operation to the underlying currency `C` (the `Balances` pallet), except:
//!
//! - `mint_into` transfers the amount from the transaction's fee payer ([`crate::EvmPayer`])
//!   instead of minting it, and fails with `CannotCreate` when there is no payer;
//! - `issue` withdraws (best effort) from the payer instead of increasing the issuance;
//! - `burn_from`, and `set_balance` downwards, withdraw and hand the credit to `B`, so every
//!   burn is counted in `Emission::TotalBurned`; `set_balance` never raises a balance;
//! - `restore`, `shelve`, `burn_held` and `burn_all_held`, which revive does not use on this
//!   runtime, fail;
//! - `deactivate` / `reactivate` do nothing: the contract ED they account for is paid, not
//!   minted, so the active issuance must not change.
//!
//! Imbalances keep the underlying currency's drop handlers, so a `Credit` of this adapter is the
//! same type as a `Credit` of `C` and can go straight to `Emission` (design D5 of m4-evm).

use core::marker::PhantomData;
use frame_support::traits::{
    Get, Imbalance as _, OnUnbalanced,
    fungible::{
        Balanced, Credit, Debt, Dust, Inspect, InspectHold, Mutate, MutateHold, Unbalanced,
        UnbalancedHold,
    },
    tokens::{
        DepositConsequence, Fortitude, Precision, Preservation, Provenance, Restriction,
        WithdrawConsequence,
    },
};
use sp_runtime::{DispatchError, DispatchResult, Saturating, TokenError};

/// A currency adapter for `pallet-revive` that never increases the total issuance.
///
/// - `C`: the underlying currency (the `Balances` pallet).
/// - `P`: the account paying for amounts revive would otherwise mint ([`crate::CurrentPayer`]).
/// - `B`: where burned credit goes (the `Emission` pallet, which counts it in `TotalBurned`).
pub struct ReviveCurrency<C, P, B>(PhantomData<(C, P, B)>);

impl<A, C: Inspect<A>, P, B> Inspect<A> for ReviveCurrency<C, P, B> {
    type Balance = C::Balance;

    fn total_issuance() -> Self::Balance {
        C::total_issuance()
    }
    fn active_issuance() -> Self::Balance {
        C::active_issuance()
    }
    fn minimum_balance() -> Self::Balance {
        C::minimum_balance()
    }
    fn total_balance(who: &A) -> Self::Balance {
        C::total_balance(who)
    }
    fn balance(who: &A) -> Self::Balance {
        C::balance(who)
    }
    fn reducible_balance(who: &A, preservation: Preservation, force: Fortitude) -> Self::Balance {
        C::reducible_balance(who, preservation, force)
    }
    fn can_deposit(who: &A, amount: Self::Balance, provenance: Provenance) -> DepositConsequence {
        C::can_deposit(who, amount, provenance)
    }
    fn can_withdraw(who: &A, amount: Self::Balance) -> WithdrawConsequence<Self::Balance> {
        C::can_withdraw(who, amount)
    }
}

impl<A, C: Unbalanced<A>, P, B> Unbalanced<A> for ReviveCurrency<C, P, B> {
    fn handle_dust(dust: Dust<A, Self>) {
        C::handle_dust(Dust(dust.0))
    }
    fn write_balance(
        who: &A,
        amount: Self::Balance,
    ) -> Result<Option<Self::Balance>, DispatchError> {
        C::write_balance(who, amount)
    }
    fn set_total_issuance(amount: Self::Balance) {
        C::set_total_issuance(amount)
    }
    fn decrease_balance(
        who: &A,
        amount: Self::Balance,
        precision: Precision,
        preservation: Preservation,
        force: Fortitude,
    ) -> Result<Self::Balance, DispatchError> {
        C::decrease_balance(who, amount, precision, preservation, force)
    }
    fn increase_balance(
        who: &A,
        amount: Self::Balance,
        precision: Precision,
    ) -> Result<Self::Balance, DispatchError> {
        C::increase_balance(who, amount, precision)
    }
    // The ED revive "mints" into a contract is paid by the transaction signer, so it is part of
    // the active issuance already; revive's deactivate/reactivate bookkeeping must not apply.
    fn deactivate(_: Self::Balance) {}
    fn reactivate(_: Self::Balance) {}
}

impl<A, C, P, B> Mutate<A> for ReviveCurrency<C, P, B>
where
    A: Eq,
    C: Mutate<A> + Balanced<A>,
    P: Get<Option<A>>,
    B: OnUnbalanced<Credit<A, C>>,
{
    /// Pays `amount` from the transaction's payer instead of minting it.
    fn mint_into(who: &A, amount: Self::Balance) -> Result<Self::Balance, DispatchError> {
        let payer = P::get().ok_or(TokenError::CannotCreate)?;
        C::transfer(&payer, who, amount, Preservation::Preserve)
    }

    /// Withdraws the amount and burns it through `B`, so it is counted.
    fn burn_from(
        who: &A,
        amount: Self::Balance,
        preservation: Preservation,
        precision: Precision,
        force: Fortitude,
    ) -> Result<Self::Balance, DispatchError> {
        let credit = C::withdraw(who, amount, precision, preservation, force)?;
        let burned = credit.peek();
        B::on_unbalanced(credit);
        Ok(burned)
    }

    fn shelve(_: &A, _: Self::Balance) -> Result<Self::Balance, DispatchError> {
        Err(TokenError::Unsupported.into())
    }

    fn restore(_: &A, _: Self::Balance) -> Result<Self::Balance, DispatchError> {
        Err(TokenError::Unsupported.into())
    }

    fn transfer(
        source: &A,
        dest: &A,
        amount: Self::Balance,
        preservation: Preservation,
    ) -> Result<Self::Balance, DispatchError> {
        C::transfer(source, dest, amount, preservation)
    }

    /// Lowers a balance by burning the difference; never raises one (that would mint).
    fn set_balance(who: &A, amount: Self::Balance) -> Self::Balance {
        let current = C::balance(who);
        if current <= amount {
            return current;
        }
        let excess = current.saturating_sub(amount);
        match Self::burn_from(
            who,
            excess,
            Preservation::Expendable,
            Precision::BestEffort,
            Fortitude::Force,
        ) {
            Ok(burned) => current.saturating_sub(burned),
            Err(_) => current,
        }
    }
}

impl<A, C, P, B> Balanced<A> for ReviveCurrency<C, P, B>
where
    C: Balanced<A>,
    P: Get<Option<A>>,
{
    type OnDropDebt = C::OnDropDebt;
    type OnDropCredit = C::OnDropCredit;

    fn rescind(amount: Self::Balance) -> Debt<A, Self> {
        C::rescind(amount)
    }

    /// Backs the credit with funds withdrawn from the payer (best effort) instead of new
    /// issuance; without a payer the credit is empty.
    fn issue(amount: Self::Balance) -> Credit<A, Self> {
        P::get()
            .and_then(|payer| {
                C::withdraw(
                    &payer,
                    amount,
                    Precision::BestEffort,
                    Preservation::Preserve,
                    Fortitude::Polite,
                )
                .ok()
            })
            .unwrap_or_else(Credit::<A, C>::zero)
    }

    fn deposit(
        who: &A,
        value: Self::Balance,
        precision: Precision,
    ) -> Result<Debt<A, Self>, DispatchError> {
        C::deposit(who, value, precision)
    }

    fn withdraw(
        who: &A,
        value: Self::Balance,
        precision: Precision,
        preservation: Preservation,
        force: Fortitude,
    ) -> Result<Credit<A, Self>, DispatchError> {
        C::withdraw(who, value, precision, preservation, force)
    }

    fn resolve(who: &A, credit: Credit<A, Self>) -> Result<(), Credit<A, Self>> {
        C::resolve(who, credit)
    }

    fn settle(
        who: &A,
        debt: Debt<A, Self>,
        preservation: Preservation,
    ) -> Result<Credit<A, Self>, Debt<A, Self>> {
        C::settle(who, debt, preservation)
    }
}

impl<A, C: InspectHold<A>, P, B> InspectHold<A> for ReviveCurrency<C, P, B> {
    type Reason = C::Reason;

    fn total_balance_on_hold(who: &A) -> Self::Balance {
        C::total_balance_on_hold(who)
    }
    fn reducible_total_balance_on_hold(who: &A, force: Fortitude) -> Self::Balance {
        C::reducible_total_balance_on_hold(who, force)
    }
    fn balance_on_hold(reason: &Self::Reason, who: &A) -> Self::Balance {
        C::balance_on_hold(reason, who)
    }
    fn hold_available(reason: &Self::Reason, who: &A) -> bool {
        C::hold_available(reason, who)
    }
    fn ensure_can_hold(reason: &Self::Reason, who: &A, amount: Self::Balance) -> DispatchResult {
        C::ensure_can_hold(reason, who, amount)
    }
    fn can_hold(reason: &Self::Reason, who: &A, amount: Self::Balance) -> bool {
        C::can_hold(reason, who, amount)
    }
}

impl<A, C: UnbalancedHold<A>, P, B> UnbalancedHold<A> for ReviveCurrency<C, P, B> {
    fn set_balance_on_hold(
        reason: &Self::Reason,
        who: &A,
        amount: Self::Balance,
    ) -> DispatchResult {
        C::set_balance_on_hold(reason, who, amount)
    }
    fn decrease_balance_on_hold(
        reason: &Self::Reason,
        who: &A,
        amount: Self::Balance,
        precision: Precision,
    ) -> Result<Self::Balance, DispatchError> {
        C::decrease_balance_on_hold(reason, who, amount, precision)
    }
    fn increase_balance_on_hold(
        reason: &Self::Reason,
        who: &A,
        amount: Self::Balance,
        precision: Precision,
    ) -> Result<Self::Balance, DispatchError> {
        C::increase_balance_on_hold(reason, who, amount, precision)
    }
}

impl<A, C, P, B> MutateHold<A> for ReviveCurrency<C, P, B>
where
    A: Eq,
    C: MutateHold<A> + Mutate<A> + Balanced<A>,
    P: Get<Option<A>>,
    B: OnUnbalanced<Credit<A, C>>,
{
    fn hold(reason: &Self::Reason, who: &A, amount: Self::Balance) -> DispatchResult {
        C::hold(reason, who, amount)
    }
    fn release(
        reason: &Self::Reason,
        who: &A,
        amount: Self::Balance,
        precision: Precision,
    ) -> Result<Self::Balance, DispatchError> {
        C::release(reason, who, amount, precision)
    }
    fn set_on_hold(reason: &Self::Reason, who: &A, amount: Self::Balance) -> DispatchResult {
        C::set_on_hold(reason, who, amount)
    }
    fn release_all(
        reason: &Self::Reason,
        who: &A,
        precision: Precision,
    ) -> Result<Self::Balance, DispatchError> {
        C::release_all(reason, who, precision)
    }
    // Burning held funds bypasses `TotalBurned`; revive only does it for PGAS deposits, which
    // this runtime does not use.
    fn burn_held(
        _: &Self::Reason,
        _: &A,
        _: Self::Balance,
        _: Precision,
        _: Fortitude,
    ) -> Result<Self::Balance, DispatchError> {
        Err(TokenError::Unsupported.into())
    }
    fn burn_all_held(
        _: &Self::Reason,
        _: &A,
        _: Precision,
        _: Fortitude,
    ) -> Result<Self::Balance, DispatchError> {
        Err(TokenError::Unsupported.into())
    }
    fn transfer_on_hold(
        reason: &Self::Reason,
        source: &A,
        dest: &A,
        amount: Self::Balance,
        precision: Precision,
        mode: Restriction,
        force: Fortitude,
    ) -> Result<Self::Balance, DispatchError> {
        C::transfer_on_hold(reason, source, dest, amount, precision, mode, force)
    }
    fn transfer_and_hold(
        reason: &Self::Reason,
        source: &A,
        dest: &A,
        amount: Self::Balance,
        precision: Precision,
        expendability: Preservation,
        force: Fortitude,
    ) -> Result<Self::Balance, DispatchError> {
        C::transfer_and_hold(
            reason,
            source,
            dest,
            amount,
            precision,
            expendability,
            force,
        )
    }
}
