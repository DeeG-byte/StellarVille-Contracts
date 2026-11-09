#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum BankError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InsufficientBalance = 3,
    LoanLimitExceeded = 4,
    UnauthorizedNotGovernor = 5,
    InvalidRate = 6,
    InsufficientReserves = 7,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Governor,
    Asset,
    BaseInterestBps,
    SavingsYieldBps,
    MinReserveRatioBps,
    TotalDeposits,
    TotalBorrows,
    Account(Address),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountRecord {
    pub deposits: i128,
    pub debt: i128,
    pub last_accrual_ledger: u32,
    pub credit_score: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BankReserves {
    pub total_deposits: i128,
    pub total_borrows: i128,
    pub min_reserve_ratio_bps: u32,
}

const DAY_IN_LEDGERS: u32 = 17_280;
const INSTANCE_BUMP_AMOUNT: u32 = 30 * DAY_IN_LEDGERS;
const INSTANCE_LIFETIME_THRESHOLD: u32 = INSTANCE_BUMP_AMOUNT - DAY_IN_LEDGERS;

#[contract]
pub struct CentralBankContract;

#[contractimpl]
impl CentralBankContract {
    pub fn initialize(
        e: Env,
        governor: Address,
        asset: Address,
        base_interest_bps: u32,
        savings_yield_bps: u32,
    ) -> Result<(), BankError> {
        if e.storage().instance().has(&DataKey::Governor) {
            return Err(BankError::AlreadyInitialized);
        }
        governor.require_auth();

        e.storage().instance().set(&DataKey::Governor, &governor);
        e.storage().instance().set(&DataKey::Asset, &asset);
        e.storage().instance().set(&DataKey::BaseInterestBps, &base_interest_bps);
        e.storage().instance().set(&DataKey::SavingsYieldBps, &savings_yield_bps);
        e.storage().instance().set(&DataKey::MinReserveRatioBps, &2000u32); // 20% default reserve ratio
        e.storage().instance().set(&DataKey::TotalDeposits, &0i128);
        e.storage().instance().set(&DataKey::TotalBorrows, &0i128);

        e.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        Ok(())
    }

    pub fn set_monetary_rates(
        e: Env,
        governor: Address,
        new_base_interest_bps: u32,
        new_savings_yield_bps: u32,
    ) -> Result<(), BankError> {
        governor.require_auth();
        let current_gov: Address = e
            .storage()
            .instance()
            .get(&DataKey::Governor)
            .ok_or(BankError::NotInitialized)?;

        if governor != current_gov {
            return Err(BankError::UnauthorizedNotGovernor);
        }

        if new_base_interest_bps > 3000 || new_savings_yield_bps > 2000 {
            return Err(BankError::InvalidRate);
        }

        e.storage()
            .instance()
            .set(&DataKey::BaseInterestBps, &new_base_interest_bps);
        e.storage()
            .instance()
            .set(&DataKey::SavingsYieldBps, &new_savings_yield_bps);

        e.events().publish(
            (symbol_short!("bank"), symbol_short!("rates")),
            (governor, new_base_interest_bps, new_savings_yield_bps),
        );
        Ok(())
    }

    pub fn deposit(e: Env, from: Address, amount: i128) -> Result<(), BankError> {
        from.require_auth();
        if amount <= 0 {
            return Err(BankError::InsufficientBalance);
        }

        let key = DataKey::Account(from.clone());
        let mut account = e.storage().persistent().get(&key).unwrap_or(AccountRecord {
            deposits: 0,
            debt: 0,
            last_accrual_ledger: e.ledger().sequence(),
            credit_score: 600,
        });

        account.deposits += amount;
        account.last_accrual_ledger = e.ledger().sequence();
        e.storage().persistent().set(&key, &account);
        e.storage()
            .persistent()
            .extend_ttl(&key, INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        let mut total_deposits: i128 = e
            .storage()
            .instance()
            .get(&DataKey::TotalDeposits)
            .unwrap_or(0);
        total_deposits += amount;
        e.storage()
            .instance()
            .set(&DataKey::TotalDeposits, &total_deposits);

        e.events().publish(
            (symbol_short!("bank"), symbol_short!("deposit")),
            (from, amount),
        );
        Ok(())
    }

    /// Withdraw savings with automatic yield accrual and minimum reserve ratio guard
    pub fn withdraw(e: Env, depositor: Address, amount: i128) -> Result<(), BankError> {
        depositor.require_auth();
        if amount <= 0 {
            return Err(BankError::InsufficientBalance);
        }

        let key = DataKey::Account(depositor.clone());
        let mut account: AccountRecord = e
            .storage()
            .persistent()
            .get(&key)
            .ok_or(BankError::InsufficientBalance)?;

        let current_ledger = e.ledger().sequence();
        let elapsed_ledgers = current_ledger.saturating_sub(account.last_accrual_ledger) as i128;
        let savings_bps: u32 = e
            .storage()
            .instance()
            .get(&DataKey::SavingsYieldBps)
            .unwrap_or(250);

        let accrued_yield = (account.deposits * (savings_bps as i128) * elapsed_ledgers)
            / (10_000 * (DAY_IN_LEDGERS as i128));
        account.deposits += accrued_yield;
        account.last_accrual_ledger = current_ledger;

        if amount > account.deposits {
            return Err(BankError::InsufficientBalance);
        }

        let mut total_deposits: i128 = e
            .storage()
            .instance()
            .get(&DataKey::TotalDeposits)
            .unwrap_or(0);
        total_deposits += accrued_yield;

        let total_borrows: i128 = e
            .storage()
            .instance()
            .get(&DataKey::TotalBorrows)
            .unwrap_or(0);
        let min_reserve_bps: u32 = e
            .storage()
            .instance()
            .get(&DataKey::MinReserveRatioBps)
            .unwrap_or(2000);

        let new_total_deposits = total_deposits - amount;
        let liquid_after = new_total_deposits - total_borrows;
        if liquid_after < 0
            || (new_total_deposits > 0
                && liquid_after * 10_000 < new_total_deposits * (min_reserve_bps as i128))
        {
            return Err(BankError::InsufficientReserves);
        }

        account.deposits -= amount;
        e.storage().persistent().set(&key, &account);
        e.storage()
            .persistent()
            .extend_ttl(&key, INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        e.storage()
            .instance()
            .set(&DataKey::TotalDeposits, &new_total_deposits);

        e.events().publish(
            (symbol_short!("bank"), symbol_short!("withdraw")),
            (depositor, amount, account.deposits),
        );
        Ok(())
    }

    pub fn borrow(e: Env, borrower: Address, amount: i128) -> Result<(), BankError> {
        borrower.require_auth();
        if amount <= 0 {
            return Err(BankError::LoanLimitExceeded);
        }

        let key = DataKey::Account(borrower.clone());
        let mut account = e.storage().persistent().get(&key).unwrap_or(AccountRecord {
            deposits: 0,
            debt: 0,
            last_accrual_ledger: e.ledger().sequence(),
            credit_score: 600,
        });

        account.debt += amount;
        account.last_accrual_ledger = e.ledger().sequence();
        e.storage().persistent().set(&key, &account);
        e.storage()
            .persistent()
            .extend_ttl(&key, INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        let mut total_borrows: i128 = e
            .storage()
            .instance()
            .get(&DataKey::TotalBorrows)
            .unwrap_or(0);
        total_borrows += amount;
        e.storage()
            .instance()
            .set(&DataKey::TotalBorrows, &total_borrows);

        e.events().publish(
            (symbol_short!("bank"), symbol_short!("borrow")),
            (borrower, amount),
        );
        Ok(())
    }

    pub fn repay(e: Env, borrower: Address, amount: i128) -> Result<(), BankError> {
        borrower.require_auth();
        let key = DataKey::Account(borrower.clone());
        let mut account: AccountRecord = e
            .storage()
            .persistent()
            .get(&key)
            .ok_or(BankError::InsufficientBalance)?;

        if amount >= account.debt {
            account.debt = 0;
            account.credit_score = (account.credit_score + 15).min(850);
        } else {
            account.debt -= amount;
        }

        e.storage().persistent().set(&key, &account);
        e.events().publish(
            (symbol_short!("bank"), symbol_short!("repay")),
            (borrower, amount, account.credit_score),
        );
        Ok(())
    }

    pub fn get_account(e: Env, account: Address) -> Option<AccountRecord> {
        e.storage().persistent().get(&DataKey::Account(account))
    }

    pub fn get_reserves(e: Env) -> BankReserves {
        BankReserves {
            total_deposits: e.storage().instance().get(&DataKey::TotalDeposits).unwrap_or(0),
            total_borrows: e.storage().instance().get(&DataKey::TotalBorrows).unwrap_or(0),
            min_reserve_ratio_bps: e
                .storage()
                .instance()
                .get(&DataKey::MinReserveRatioBps)
                .unwrap_or(2000),
        }
    }

    pub fn get_rates(e: Env) -> (u32, u32) {
        let borrow_bps: u32 = e
            .storage()
            .instance()
            .get(&DataKey::BaseInterestBps)
            .unwrap_or(400);
        let savings_bps: u32 = e
            .storage()
            .instance()
            .get(&DataKey::SavingsYieldBps)
            .unwrap_or(250);
        (borrow_bps, savings_bps)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_withdraw_and_reserve_ratio_guard() {
        let e = Env::default();
        e.mock_all_auths();

        let contract_id = e.register_contract(None, CentralBankContract);
        let client = CentralBankContractClient::new(&e, &contract_id);

        let gov = Address::generate(&e);
        let asset = Address::generate(&e);
        let alice = Address::generate(&e);
        let bob = Address::generate(&e);

        client.initialize(&gov, &asset, &400, &250);
        client.deposit(&alice, &1000);

        // Valid partial withdrawal with 0 borrows
        client.withdraw(&alice, &200);
        let acct = client.get_account(&alice).unwrap();
        assert_eq!(acct.deposits, 800);

        // Bob borrows 600 out of 800 total deposits (leaving 200 liquid = 25% reserve)
        client.borrow(&bob, &600);

        // Withdrawing 100 would leave 700 deposits and 100 liquid (14.2% < 20% min reserve), should fail
        let res = client.try_withdraw(&alice, &100);
        assert!(res.is_err());
    }
}
