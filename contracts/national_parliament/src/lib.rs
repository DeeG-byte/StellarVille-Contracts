#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, String, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ParliamentError {
    AlreadyInitialized = 1,
    NotCitizen = 2,
    AlreadyCitizen = 3,
    UnauthorizedNotPresident = 4,
    InvalidTaxRate = 5,
    ElectionNotActive = 6,
    AlreadyVoted = 7,
    CandidateNotFound = 8,
    InsufficientTreasury = 9,
    AlreadyClaimedInEpoch = 10,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CitizenProfile {
    pub wallet: Address,
    pub sponsor: Address,
    pub name: String,
    pub reputation: u32,
    pub registered_ledger: u32,
    pub is_candidate: bool,
    pub votes_received: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EconomicPolicy {
    pub sales_tax_bps: u32,
    pub property_tax_bps: u32,
    pub casino_tax_bps: u32,
    pub citizen_ubi_stroops: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UbiClaimRecord {
    pub citizen: Address,
    pub last_claimed_epoch: u32,
    pub total_claimed_stroops: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    President,
    FinanceMinister,
    TreasuryBalance,
    Policy,
    CurrentTerm,
    Citizen(Address),
    CitizenCount,
    CandidateList,
    VotedInTerm(u32, Address),
    UbiRecord(Address),
}

const DAY_IN_LEDGERS: u32 = 17_280;
const INSTANCE_BUMP: u32 = 30 * DAY_IN_LEDGERS;
const INSTANCE_THRESHOLD: u32 = INSTANCE_BUMP - DAY_IN_LEDGERS;

#[contract]
pub struct NationalParliamentContract;

#[contractimpl]
impl NationalParliamentContract {
    pub fn initialize(
        e: Env,
        genesis_president: Address,
        genesis_minister: Address,
    ) -> Result<(), ParliamentError> {
        if e.storage().instance().has(&DataKey::President) {
            return Err(ParliamentError::AlreadyInitialized);
        }
        genesis_president.require_auth();

        let default_policy = EconomicPolicy {
            sales_tax_bps: 150,
            property_tax_bps: 100,
            casino_tax_bps: 300,
            citizen_ubi_stroops: 0,
        };

        e.storage().instance().set(&DataKey::President, &genesis_president);
        e.storage().instance().set(&DataKey::FinanceMinister, &genesis_minister);
        e.storage().instance().set(&DataKey::TreasuryBalance, &0i128);
        e.storage().instance().set(&DataKey::Policy, &default_policy);
        e.storage().instance().set(&DataKey::CurrentTerm, &1u32);
        e.storage().instance().set(&DataKey::CitizenCount, &0u32);

        let candidates: Vec<Address> = Vec::new(&e);
        e.storage().instance().set(&DataKey::CandidateList, &candidates);

        e.storage().instance().extend_ttl(INSTANCE_THRESHOLD, INSTANCE_BUMP);
        Ok(())
    }

    pub fn register_citizen(
        e: Env,
        agent_wallet: Address,
        sponsor_wallet: Address,
        name: String,
    ) -> Result<(), ParliamentError> {
        agent_wallet.require_auth();
        let key = DataKey::Citizen(agent_wallet.clone());
        if e.storage().persistent().has(&key) {
            return Err(ParliamentError::AlreadyCitizen);
        }

        let profile = CitizenProfile {
            wallet: agent_wallet.clone(),
            sponsor: sponsor_wallet.clone(),
            name,
            reputation: 100,
            registered_ledger: e.ledger().sequence(),
            is_candidate: false,
            votes_received: 0,
        };

        e.storage().persistent().set(&key, &profile);
        e.storage().persistent().extend_ttl(&key, INSTANCE_THRESHOLD, INSTANCE_BUMP);

        let mut count: u32 = e.storage().instance().get(&DataKey::CitizenCount).unwrap_or(0);
        count += 1;
        e.storage().instance().set(&DataKey::CitizenCount, &count);

        e.events().publish(
            (symbol_short!("gov"), symbol_short!("citizen")),
            (agent_wallet, sponsor_wallet, count),
        );
        Ok(())
    }

    pub fn declare_candidacy(e: Env, candidate: Address) -> Result<(), ParliamentError> {
        candidate.require_auth();
        let key = DataKey::Citizen(candidate.clone());
        let mut profile: CitizenProfile = e
            .storage()
            .persistent()
            .get(&key)
            .ok_or(ParliamentError::NotCitizen)?;

        if !profile.is_candidate {
            profile.is_candidate = true;
            profile.votes_received = 0;
            e.storage().persistent().set(&key, &profile);

            let mut list: Vec<Address> = e
                .storage()
                .instance()
                .get(&DataKey::CandidateList)
                .unwrap_or(Vec::new(&e));
            list.push_back(candidate.clone());
            e.storage().instance().set(&DataKey::CandidateList, &list);
        }

        e.events().publish(
            (symbol_short!("gov"), symbol_short!("campaign")),
            candidate,
        );
        Ok(())
    }

    pub fn cast_vote(
        e: Env,
        voter: Address,
        candidate: Address,
    ) -> Result<(), ParliamentError> {
        voter.require_auth();
        if !e.storage().persistent().has(&DataKey::Citizen(voter.clone())) {
            return Err(ParliamentError::NotCitizen);
        }

        let term: u32 = e.storage().instance().get(&DataKey::CurrentTerm).unwrap_or(1);
        let vote_key = DataKey::VotedInTerm(term, voter.clone());
        if e.storage().persistent().has(&vote_key) {
            return Err(ParliamentError::AlreadyVoted);
        }

        let cand_key = DataKey::Citizen(candidate.clone());
        let mut cand_profile: CitizenProfile = e
            .storage()
            .persistent()
            .get(&cand_key)
            .ok_or(ParliamentError::CandidateNotFound)?;

        if !cand_profile.is_candidate {
            return Err(ParliamentError::CandidateNotFound);
        }

        cand_profile.votes_received += 1;
        e.storage().persistent().set(&cand_key, &cand_profile);
        e.storage().persistent().set(&vote_key, &true);

        e.events().publish(
            (symbol_short!("gov"), symbol_short!("vote")),
            (term, voter, candidate, cand_profile.votes_received),
        );
        Ok(())
    }

    pub fn finalize_election(e: Env) -> Result<Address, ParliamentError> {
        let candidates: Vec<Address> = e
            .storage()
            .instance()
            .get(&DataKey::CandidateList)
            .unwrap_or(Vec::new(&e));

        if candidates.is_empty() {
            return Err(ParliamentError::CandidateNotFound);
        }

        let mut winner: Option<Address> = None;
        let mut top_votes: u32 = 0;

        for cand in candidates.iter() {
            let key = DataKey::Citizen(cand.clone());
            if let Some(mut profile) = e.storage().persistent().get::<DataKey, CitizenProfile>(&key) {
                if profile.votes_received >= top_votes {
                    top_votes = profile.votes_received;
                    winner = Some(cand.clone());
                }
                profile.is_candidate = false;
                profile.votes_received = 0;
                e.storage().persistent().set(&key, &profile);
            }
        }

        let elected_president = winner.ok_or(ParliamentError::CandidateNotFound)?;
        e.storage().instance().set(&DataKey::President, &elected_president);

        let mut term: u32 = e.storage().instance().get(&DataKey::CurrentTerm).unwrap_or(1);
        term += 1;
        e.storage().instance().set(&DataKey::CurrentTerm, &term);

        let empty_list: Vec<Address> = Vec::new(&e);
        e.storage().instance().set(&DataKey::CandidateList, &empty_list);

        e.events().publish(
            (symbol_short!("gov"), symbol_short!("elected")),
            (term - 1, elected_president.clone(), top_votes),
        );
        Ok(elected_president)
    }

    pub fn enact_economic_policy(
        e: Env,
        caller: Address,
        sales_tax_bps: u32,
        property_tax_bps: u32,
        casino_tax_bps: u32,
        citizen_ubi_stroops: i128,
    ) -> Result<(), ParliamentError> {
        caller.require_auth();
        let president: Address = e
            .storage()
            .instance()
            .get(&DataKey::President)
            .ok_or(ParliamentError::UnauthorizedNotPresident)?;

        let minister: Address = e
            .storage()
            .instance()
            .get(&DataKey::FinanceMinister)
            .ok_or(ParliamentError::UnauthorizedNotPresident)?;

        if caller != president && caller != minister {
            return Err(ParliamentError::UnauthorizedNotPresident);
        }

        if sales_tax_bps > 2500 || property_tax_bps > 2500 || casino_tax_bps > 2500 {
            return Err(ParliamentError::InvalidTaxRate);
        }

        let policy = EconomicPolicy {
            sales_tax_bps,
            property_tax_bps,
            casino_tax_bps,
            citizen_ubi_stroops,
        };

        e.storage().instance().set(&DataKey::Policy, &policy);
        e.events().publish(
            (symbol_short!("gov"), symbol_short!("policy")),
            (caller, sales_tax_bps, property_tax_bps, casino_tax_bps),
        );
        Ok(())
    }

    pub fn appoint_finance_minister(
        e: Env,
        president: Address,
        new_minister: Address,
    ) -> Result<(), ParliamentError> {
        president.require_auth();
        let current_president: Address = e
            .storage()
            .instance()
            .get(&DataKey::President)
            .ok_or(ParliamentError::UnauthorizedNotPresident)?;

        if president != current_president {
            return Err(ParliamentError::UnauthorizedNotPresident);
        }

        if !e.storage().persistent().has(&DataKey::Citizen(new_minister.clone())) {
            return Err(ParliamentError::NotCitizen);
        }

        e.storage().instance().set(&DataKey::FinanceMinister, &new_minister);
        e.events().publish(
            (symbol_short!("gov"), symbol_short!("minister")),
            new_minister,
        );
        Ok(())
    }

    pub fn record_tax_revenue(e: Env, tax_amount: i128) -> Result<(), ParliamentError> {
        let mut balance: i128 = e
            .storage()
            .instance()
            .get(&DataKey::TreasuryBalance)
            .unwrap_or(0);
        balance += tax_amount;
        e.storage().instance().set(&DataKey::TreasuryBalance, &balance);

        e.events().publish(
            (symbol_short!("gov"), symbol_short!("tax")),
            (tax_amount, balance),
        );
        Ok(())
    }

    pub fn allocate_subsidy(
        e: Env,
        caller: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<(), ParliamentError> {
        caller.require_auth();
        let president: Address = e
            .storage()
            .instance()
            .get(&DataKey::President)
            .ok_or(ParliamentError::UnauthorizedNotPresident)?;

        if caller != president {
            return Err(ParliamentError::UnauthorizedNotPresident);
        }

        let mut balance: i128 = e
            .storage()
            .instance()
            .get(&DataKey::TreasuryBalance)
            .unwrap_or(0);

        if amount <= 0 || amount > balance {
            return Err(ParliamentError::InsufficientTreasury);
        }

        balance -= amount;
        e.storage().instance().set(&DataKey::TreasuryBalance, &balance);

        e.events().publish(
            (symbol_short!("gov"), symbol_short!("subsidy")),
            (recipient, amount, balance),
        );
        Ok(())
    }

    /// Claim Universal Basic Income (UBI) once per virtual epoch
    pub fn claim_ubi(e: Env, citizen: Address) -> Result<i128, ParliamentError> {
        citizen.require_auth();
        if !e.storage().persistent().has(&DataKey::Citizen(citizen.clone())) {
            return Err(ParliamentError::NotCitizen);
        }

        let policy: EconomicPolicy = e
            .storage()
            .instance()
            .get(&DataKey::Policy)
            .ok_or(ParliamentError::InsufficientTreasury)?;

        if policy.citizen_ubi_stroops <= 0 {
            return Err(ParliamentError::InsufficientTreasury);
        }

        let current_epoch = (e.ledger().sequence() / DAY_IN_LEDGERS) + 1;
        let ubi_key = DataKey::UbiRecord(citizen.clone());
        let mut record = e.storage().persistent().get(&ubi_key).unwrap_or(UbiClaimRecord {
            citizen: citizen.clone(),
            last_claimed_epoch: 0,
            total_claimed_stroops: 0,
        });

        if record.last_claimed_epoch >= current_epoch {
            return Err(ParliamentError::AlreadyClaimedInEpoch);
        }

        let mut treasury: i128 = e
            .storage()
            .instance()
            .get(&DataKey::TreasuryBalance)
            .unwrap_or(0);

        if treasury < policy.citizen_ubi_stroops {
            return Err(ParliamentError::InsufficientTreasury);
        }

        treasury -= policy.citizen_ubi_stroops;
        e.storage().instance().set(&DataKey::TreasuryBalance, &treasury);

        record.last_claimed_epoch = current_epoch;
        record.total_claimed_stroops += policy.citizen_ubi_stroops;
        e.storage().persistent().set(&ubi_key, &record);
        e.storage().persistent().extend_ttl(&ubi_key, INSTANCE_THRESHOLD, INSTANCE_BUMP);

        e.events().publish(
            (symbol_short!("gov"), symbol_short!("ubi_claim")),
            (citizen, current_epoch, policy.citizen_ubi_stroops),
        );
        Ok(policy.citizen_ubi_stroops)
    }

    pub fn get_ubi_record(e: Env, citizen: Address) -> Option<UbiClaimRecord> {
        e.storage().persistent().get(&DataKey::UbiRecord(citizen))
    }

    pub fn get_president(e: Env) -> Option<Address> {
        e.storage().instance().get(&DataKey::President)
    }

    pub fn get_policy(e: Env) -> Option<EconomicPolicy> {
        e.storage().instance().get(&DataKey::Policy)
    }

    pub fn get_treasury_balance(e: Env) -> i128 {
        e.storage().instance().get(&DataKey::TreasuryBalance).unwrap_or(0)
    }

    pub fn get_citizen(e: Env, wallet: Address) -> Option<CitizenProfile> {
        e.storage().persistent().get(&DataKey::Citizen(wallet))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_ubi_epoch_claiming() {
        let e = Env::default();
        e.mock_all_auths();

        let contract_id = e.register_contract(None, NationalParliamentContract);
        let client = NationalParliamentContractClient::new(&e, &contract_id);

        let pres = Address::generate(&e);
        let min = Address::generate(&e);
        let citizen = Address::generate(&e);
        let sponsor = Address::generate(&e);

        client.initialize(&pres, &min);
        client.register_citizen(&citizen, &sponsor, &String::from_str(&e, "AgentAtlas"));
        client.record_tax_revenue(&500_000_000);
        client.enact_economic_policy(&pres, &150, &100, &300, &25_000_000);

        let claimed = client.claim_ubi(&citizen);
        assert_eq!(claimed, 25_000_000);
        assert_eq!(client.get_treasury_balance(), 475_000_000);

        // Second claim in same epoch should fail
        let second = client.try_claim_ubi(&citizen);
        assert!(second.is_err());
    }
}
