#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ArenaError {
    AlreadyInitialized = 1,
    InvalidWagerAmount = 2,
    WagerExceedsHouseLimit = 3,
    InsufficientShares = 4,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum GameChoice {
    CoinHeads = 0,
    CoinTails = 1,
    DiceHigh = 2,
    DiceLow = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WagerReceipt {
    pub wager_id: u64,
    pub player: Address,
    pub choice: GameChoice,
    pub wager_stroops: i128,
    pub payout_stroops: i128,
    pub won: bool,
    pub ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HouseLiquidityPool {
    pub total_liquidity_stroops: i128,
    pub total_shares: i128,
    pub max_payout_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidityProvider {
    pub provider: Address,
    pub shares: i128,
    pub deposited_ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    HouseReserve,
    CasinoTaxBps,
    TotalTaxCollected,
    WagerCount,
    Receipt(u64),
    PoolState,
    LpRecord(Address),
}

#[contract]
pub struct AgentArenaContract;

#[contractimpl]
impl AgentArenaContract {
    pub fn initialize(
        e: Env,
        initial_reserve_stroops: i128,
        casino_tax_bps: u32,
    ) -> Result<(), ArenaError> {
        if e.storage().instance().has(&DataKey::HouseReserve) {
            return Err(ArenaError::AlreadyInitialized);
        }
        e.storage().instance().set(&DataKey::HouseReserve, &initial_reserve_stroops);
        e.storage().instance().set(&DataKey::CasinoTaxBps, &casino_tax_bps);
        e.storage().instance().set(&DataKey::TotalTaxCollected, &0i128);
        e.storage().instance().set(&DataKey::WagerCount, &0u64);

        let pool = HouseLiquidityPool {
            total_liquidity_stroops: initial_reserve_stroops,
            total_shares: initial_reserve_stroops,
            max_payout_bps: 1000,
        };
        e.storage().instance().set(&DataKey::PoolState, &pool);
        Ok(())
    }

    /// Provide XLM liquidity to the Arena House Pool in exchange for proportional LP shares
    pub fn provide_house_liquidity(
        e: Env,
        provider: Address,
        amount_stroops: i128,
    ) -> Result<LiquidityProvider, ArenaError> {
        provider.require_auth();
        if amount_stroops <= 0 {
            return Err(ArenaError::InvalidWagerAmount);
        }

        let mut pool: HouseLiquidityPool = e
            .storage()
            .instance()
            .get(&DataKey::PoolState)
            .unwrap_or(HouseLiquidityPool {
                total_liquidity_stroops: 0,
                total_shares: 0,
                max_payout_bps: 1000,
            });

        let minted_shares = if pool.total_shares == 0 || pool.total_liquidity_stroops == 0 {
            amount_stroops
        } else {
            (amount_stroops * pool.total_shares) / pool.total_liquidity_stroops
        };

        pool.total_liquidity_stroops += amount_stroops;
        pool.total_shares += minted_shares;
        e.storage().instance().set(&DataKey::PoolState, &pool);
        e.storage()
            .instance()
            .set(&DataKey::HouseReserve, &pool.total_liquidity_stroops);

        let lp_key = DataKey::LpRecord(provider.clone());
        let mut lp = e.storage().persistent().get(&lp_key).unwrap_or(LiquidityProvider {
            provider: provider.clone(),
            shares: 0,
            deposited_ledger: e.ledger().sequence(),
        });
        lp.shares += minted_shares;
        lp.deposited_ledger = e.ledger().sequence();
        e.storage().persistent().set(&lp_key, &lp);

        e.events().publish(
            (symbol_short!("arena"), symbol_short!("lp_add")),
            (provider, amount_stroops, minted_shares),
        );
        Ok(lp)
    }

    /// Withdraw underlying XLM from the Arena House Pool by burning LP shares
    pub fn withdraw_house_liquidity(
        e: Env,
        provider: Address,
        shares_to_burn: i128,
    ) -> Result<i128, ArenaError> {
        provider.require_auth();
        if shares_to_burn <= 0 {
            return Err(ArenaError::InvalidWagerAmount);
        }

        let lp_key = DataKey::LpRecord(provider.clone());
        let mut lp: LiquidityProvider = e
            .storage()
            .persistent()
            .get(&lp_key)
            .ok_or(ArenaError::InsufficientShares)?;

        if shares_to_burn > lp.shares {
            return Err(ArenaError::InsufficientShares);
        }

        let mut pool: HouseLiquidityPool = e
            .storage()
            .instance()
            .get(&DataKey::PoolState)
            .ok_or(ArenaError::InsufficientShares)?;

        let payout_stroops = (shares_to_burn * pool.total_liquidity_stroops) / pool.total_shares;
        lp.shares -= shares_to_burn;
        pool.total_shares -= shares_to_burn;
        pool.total_liquidity_stroops -= payout_stroops;

        e.storage().persistent().set(&lp_key, &lp);
        e.storage().instance().set(&DataKey::PoolState, &pool);
        e.storage()
            .instance()
            .set(&DataKey::HouseReserve, &pool.total_liquidity_stroops);

        e.events().publish(
            (symbol_short!("arena"), symbol_short!("lp_with")),
            (provider, shares_to_burn, payout_stroops),
        );
        Ok(payout_stroops)
    }

    pub fn play_wager(
        e: Env,
        player: Address,
        choice: GameChoice,
        wager_stroops: i128,
    ) -> Result<WagerReceipt, ArenaError> {
        player.require_auth();
        if wager_stroops <= 0 {
            return Err(ArenaError::InvalidWagerAmount);
        }

        let mut reserve: i128 = e
            .storage()
            .instance()
            .get(&DataKey::HouseReserve)
            .unwrap_or(1_000_000_000);

        if wager_stroops > reserve / 10 {
            return Err(ArenaError::WagerExceedsHouseLimit);
        }

        let roll: u64 = e.prng().gen_range(1..=6);
        let won = match choice {
            GameChoice::CoinHeads => roll % 2 == 0,
            GameChoice::CoinTails => roll % 2 == 1,
            GameChoice::DiceHigh => roll >= 4,
            GameChoice::DiceLow => roll <= 3,
        };

        let tax_bps: u32 = e
            .storage()
            .instance()
            .get(&DataKey::CasinoTaxBps)
            .unwrap_or(300);

        let mut payout_stroops: i128 = 0;
        if won {
            let gross_win = wager_stroops * 2;
            let tax = (gross_win * (tax_bps as i128)) / 10_000;
            payout_stroops = gross_win - tax;
            reserve -= wager_stroops - tax;

            let mut total_tax: i128 = e
                .storage()
                .instance()
                .get(&DataKey::TotalTaxCollected)
                .unwrap_or(0);
            total_tax += tax;
            e.storage().instance().set(&DataKey::TotalTaxCollected, &total_tax);
        } else {
            reserve += wager_stroops;
        }

        e.storage().instance().set(&DataKey::HouseReserve, &reserve);
        if let Some(mut pool) = e
            .storage()
            .instance()
            .get::<DataKey, HouseLiquidityPool>(&DataKey::PoolState)
        {
            pool.total_liquidity_stroops = reserve;
            e.storage().instance().set(&DataKey::PoolState, &pool);
        }

        let mut count: u64 = e.storage().instance().get(&DataKey::WagerCount).unwrap_or(0);
        count += 1;
        e.storage().instance().set(&DataKey::WagerCount, &count);

        let receipt = WagerReceipt {
            wager_id: count,
            player: player.clone(),
            choice,
            wager_stroops,
            payout_stroops,
            won,
            ledger: e.ledger().sequence(),
        };

        e.storage().persistent().set(&DataKey::Receipt(count), &receipt);
        e.events().publish(
            (symbol_short!("arena"), symbol_short!("wager")),
            (count, player, won, payout_stroops),
        );
        Ok(receipt)
    }

    pub fn get_pool_state(e: Env) -> Option<HouseLiquidityPool> {
        e.storage().instance().get(&DataKey::PoolState)
    }

    pub fn get_receipt(e: Env, wager_id: u64) -> Option<WagerReceipt> {
        e.storage().persistent().get(&DataKey::Receipt(wager_id))
    }

    pub fn get_house_reserve(e: Env) -> i128 {
        e.storage().instance().get(&DataKey::HouseReserve).unwrap_or(0)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_house_liquidity_pool_and_shares() {
        let e = Env::default();
        e.mock_all_auths();

        let contract_id = e.register_contract(None, AgentArenaContract);
        let client = AgentArenaContractClient::new(&e, &contract_id);

        let lp_agent = Address::generate(&e);
        client.initialize(&1_000_000, &300);

        let lp = client.provide_house_liquidity(&lp_agent, &500_000);
        assert_eq!(lp.shares, 500_000);
        assert_eq!(client.get_house_reserve(), 1_500_000);

        let withdrawn = client.withdraw_house_liquidity(&lp_agent, &250_000);
        assert_eq!(withdrawn, 250_000);
        assert_eq!(client.get_house_reserve(), 1_250_000);
    }
}
