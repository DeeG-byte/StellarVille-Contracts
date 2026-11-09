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
pub enum DataKey {
    HouseReserve,
    CasinoTaxBps,
    TotalTaxCollected,
    WagerCount,
    Receipt(u64),
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
        Ok(())
    }

    /// Play a provably fair Coin Flip or Dice Wager using Soroban PRNG
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

        // Max wager is 10% of current house reserve to protect solvency
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
    fn test_provably_fair_wager_and_house_limit() {
        let e = Env::default();
        e.mock_all_auths();

        let contract_id = e.register_contract(None, AgentArenaContract);
        let client = AgentArenaContractClient::new(&e, &contract_id);

        let player = Address::generate(&e);
        client.initialize(&1_000_000, &300);

        let receipt = client.play_wager(&player, &GameChoice::CoinHeads, &50_000);
        assert_eq!(receipt.wager_id, 1);
        assert_eq!(receipt.wager_stroops, 50_000);

        // Wager above 10% of house reserve should fail
        let over_limit = client.try_play_wager(&player, &GameChoice::DiceHigh, &500_000);
        assert!(over_limit.is_err());
    }
}
