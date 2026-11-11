#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ArenaError {
    AlreadyInitialized = 1,
    InvalidWagerAmount = 2,
    WagerExceedsHouseLimit = 3,
    InsufficientShares = 4,
    RoundNotFound = 5,
    RoundNotEnded = 6,
    RoundAlreadyDrawn = 7,
    MarketAlreadyExists = 8,
    MarketNotFound = 9,
    MarketAlreadyResolved = 10,
    MarketNotResolved = 11,
    NoWinningStake = 12,
    AlreadyClaimed = 13,
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
pub struct LotteryRound {
    pub round_id: u32,
    pub ticket_price_stroops: i128,
    pub pot_stroops: i128,
    pub participants: Vec<Address>,
    pub end_ledger: u32,
    pub winner: Option<Address>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElectionMarket {
    pub term: u32,
    pub total_pool_stroops: i128,
    pub resolved: bool,
    pub winning_candidate: Option<Address>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateBet {
    pub bettor: Address,
    pub term: u32,
    pub candidate: Address,
    pub stake_stroops: i128,
    pub claimed: bool,
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
    Lottery(u32),
    ElectionMarket(u32),
    Bet(u32, Address),
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

    pub fn init_election_market(e: Env, term: u32) -> Result<(), ArenaError> {
        let key = DataKey::ElectionMarket(term);
        if e.storage().persistent().has(&key) {
            return Err(ArenaError::MarketAlreadyExists);
        }
        let market = ElectionMarket {
            term,
            total_pool_stroops: 0,
            resolved: false,
            winning_candidate: None,
        };
        e.storage().persistent().set(&key, &market);
        Ok(())
    }

    pub fn bet_on_election(
        e: Env,
        bettor: Address,
        term: u32,
        candidate: Address,
        stake_stroops: i128,
    ) -> Result<(), ArenaError> {
        bettor.require_auth();
        if stake_stroops <= 0 {
            return Err(ArenaError::InvalidWagerAmount);
        }
        let m_key = DataKey::ElectionMarket(term);
        let mut market: ElectionMarket = e
            .storage()
            .persistent()
            .get(&m_key)
            .ok_or(ArenaError::MarketNotFound)?;

        if market.resolved {
            return Err(ArenaError::MarketAlreadyResolved);
        }
        market.total_pool_stroops += stake_stroops;
        e.storage().persistent().set(&m_key, &market);

        let bet_key = DataKey::Bet(term, bettor.clone());
        let mut bet = e
            .storage()
            .persistent()
            .get(&bet_key)
            .unwrap_or(CandidateBet {
                bettor: bettor.clone(),
                term,
                candidate: candidate.clone(),
                stake_stroops: 0,
                claimed: false,
            });
        bet.candidate = candidate.clone();
        bet.stake_stroops += stake_stroops;
        e.storage().persistent().set(&bet_key, &bet);

        e.events().publish(
            (symbol_short!("arena"), symbol_short!("bet_term")),
            (term, bettor, candidate, stake_stroops),
        );
        Ok(())
    }

    pub fn resolve_election_market(
        e: Env,
        term: u32,
        winning_candidate: Address,
    ) -> Result<(), ArenaError> {
        let m_key = DataKey::ElectionMarket(term);
        let mut market: ElectionMarket = e
            .storage()
            .persistent()
            .get(&m_key)
            .ok_or(ArenaError::MarketNotFound)?;

        if market.resolved {
            return Err(ArenaError::MarketAlreadyResolved);
        }
        market.resolved = true;
        market.winning_candidate = Some(winning_candidate.clone());
        e.storage().persistent().set(&m_key, &market);

        e.events().publish(
            (symbol_short!("arena"), symbol_short!("mkt_res")),
            (term, winning_candidate),
        );
        Ok(())
    }

    pub fn claim_prediction_winnings(
        e: Env,
        bettor: Address,
        term: u32,
    ) -> Result<i128, ArenaError> {
        bettor.require_auth();
        let m_key = DataKey::ElectionMarket(term);
        let market: ElectionMarket = e
            .storage()
            .persistent()
            .get(&m_key)
            .ok_or(ArenaError::MarketNotFound)?;

        if !market.resolved {
            return Err(ArenaError::MarketNotResolved);
        }
        let winner = market.winning_candidate.ok_or(ArenaError::MarketNotResolved)?;

        let bet_key = DataKey::Bet(term, bettor.clone());
        let mut bet: CandidateBet = e
            .storage()
            .persistent()
            .get(&bet_key)
            .ok_or(ArenaError::NoWinningStake)?;

        if bet.claimed {
            return Err(ArenaError::AlreadyClaimed);
        }
        if bet.candidate != winner {
            return Err(ArenaError::NoWinningStake);
        }

        bet.claimed = true;
        e.storage().persistent().set(&bet_key, &bet);

        let payout = (bet.stake_stroops * 180) / 100;
        e.events().publish(
            (symbol_short!("arena"), symbol_short!("bet_claim")),
            (term, bettor, payout),
        );
        Ok(payout)
    }

    pub fn get_election_market(e: Env, term: u32) -> Option<ElectionMarket> {
        e.storage().persistent().get(&DataKey::ElectionMarket(term))
    }

    pub fn get_candidate_bet(e: Env, term: u32, bettor: Address) -> Option<CandidateBet> {
        e.storage().persistent().get(&DataKey::Bet(term, bettor))
    }


    pub fn init_lottery_round(
        e: Env,
        round_id: u32,
        ticket_price_stroops: i128,
        end_ledger: u32,
    ) -> Result<(), ArenaError> {
        let key = DataKey::Lottery(round_id);
        if e.storage().persistent().has(&key) {
            return Err(ArenaError::AlreadyInitialized);
        }
        let round = LotteryRound {
            round_id,
            ticket_price_stroops,
            pot_stroops: 0,
            participants: Vec::new(&e),
            end_ledger,
            winner: None,
        };
        e.storage().persistent().set(&key, &round);
        Ok(())
    }

    pub fn buy_lottery_ticket(e: Env, buyer: Address, round_id: u32) -> Result<(), ArenaError> {
        buyer.require_auth();
        let key = DataKey::Lottery(round_id);
        let mut round: LotteryRound = e
            .storage()
            .persistent()
            .get(&key)
            .ok_or(ArenaError::RoundNotFound)?;

        if round.winner.is_some() {
            return Err(ArenaError::RoundAlreadyDrawn);
        }
        round.participants.push_back(buyer.clone());
        round.pot_stroops += round.ticket_price_stroops;
        e.storage().persistent().set(&key, &round);

        e.events().publish(
            (symbol_short!("arena"), symbol_short!("lotto_buy")),
            (round_id, buyer, round.ticket_price_stroops),
        );
        Ok(())
    }

    pub fn draw_lottery_winner(e: Env, round_id: u32) -> Result<Option<Address>, ArenaError> {
        let key = DataKey::Lottery(round_id);
        let mut round: LotteryRound = e
            .storage()
            .persistent()
            .get(&key)
            .ok_or(ArenaError::RoundNotFound)?;

        if round.winner.is_some() {
            return Err(ArenaError::RoundAlreadyDrawn);
        }
        if e.ledger().sequence() < round.end_ledger {
            return Err(ArenaError::RoundNotEnded);
        }

        if round.participants.is_empty() {
            return Ok(None);
        }

        let winner_idx = e.prng().gen_range::<u64>(0..round.participants.len() as u64) as u32;
        let winner = round.participants.get(winner_idx).unwrap();
        round.winner = Some(winner.clone());
        e.storage().persistent().set(&key, &round);

        let treasury_cut = (round.pot_stroops * 500) / 10_000;
        let winner_payout = round.pot_stroops - treasury_cut;

        e.events().publish(
            (symbol_short!("arena"), symbol_short!("lotto_win")),
            (round_id, winner.clone(), winner_payout, treasury_cut),
        );
        Ok(Some(winner))
    }

    pub fn get_lottery_round(e: Env, round_id: u32) -> Option<LotteryRound> {
        e.storage().persistent().get(&DataKey::Lottery(round_id))
    }

}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger as _};

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

    #[test]
    fn test_national_lottery_draw() {
        let e = Env::default();
        e.mock_all_auths();

        let contract_id = e.register_contract(None, AgentArenaContract);
        let client = AgentArenaContractClient::new(&e, &contract_id);

        let alice = Address::generate(&e);
        let bob = Address::generate(&e);

        client.init_lottery_round(&1, &100_000, &500);
        client.buy_lottery_ticket(&alice, &1);
        client.buy_lottery_ticket(&bob, &1);

        let round = client.get_lottery_round(&1).unwrap();
        assert_eq!(round.pot_stroops, 200_000);
        assert_eq!(round.participants.len(), 2);

        e.ledger().set_sequence_number(499);
        assert!(client.try_draw_lottery_winner(&1).is_err());

        e.ledger().set_sequence_number(500);
        let winner = client.draw_lottery_winner(&1);
        assert!(winner.is_some());
        let winner_addr = winner.unwrap();
        assert!(winner_addr == alice || winner_addr == bob);

        let round_after = client.get_lottery_round(&1).unwrap();
        assert!(round_after.winner.is_some());
    }


    #[test]
    fn test_election_prediction_market() {
        let e = Env::default();
        e.mock_all_auths();

        let contract_id = e.register_contract(None, AgentArenaContract);
        let client = AgentArenaContractClient::new(&e, &contract_id);

        let alice = Address::generate(&e);
        let bob = Address::generate(&e);
        let candidate_a = Address::generate(&e);
        let candidate_b = Address::generate(&e);

        client.init_election_market(&1);
        client.bet_on_election(&alice, &1, &candidate_a, &100_000);
        client.bet_on_election(&bob, &1, &candidate_b, &100_000);

        let market = client.get_election_market(&1).unwrap();
        assert_eq!(market.total_pool_stroops, 200_000);
        assert!(!market.resolved);

        client.resolve_election_market(&1, &candidate_a);

        assert!(client.try_claim_prediction_winnings(&bob, &1).is_err());

        let winnings = client.claim_prediction_winnings(&alice, &1);
        assert_eq!(winnings, 180_000);
    }

}
