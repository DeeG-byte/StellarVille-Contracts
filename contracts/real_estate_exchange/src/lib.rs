#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, String,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MarketError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    PropertyNotFound = 3,
    PropertyNotForSale = 4,
    UnauthorizedOwner = 5,
    InvalidPrice = 6,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PropertyType {
    Bakery = 0,
    MediaOutlet = 1,
    Casino = 2,
    Residential = 3,
    CommercialHub = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PropertyDeed {
    pub property_id: u32,
    pub owner: Address,
    pub property_type: PropertyType,
    pub name: String,
    pub price_stroops: i128,
    pub for_sale: bool,
    pub yield_bps: u32,
    pub last_tax_ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    Asset,
    Treasury,
    PropertyTaxBps,
    PropertyCount,
    Deed(u32),
}

const DAY_IN_LEDGERS: u32 = 17_280;
const INSTANCE_BUMP: u32 = 30 * DAY_IN_LEDGERS;
const INSTANCE_THRESHOLD: u32 = INSTANCE_BUMP - DAY_IN_LEDGERS;

#[contract]
pub struct RealEstateExchangeContract;

#[contractimpl]
impl RealEstateExchangeContract {
    pub fn initialize(
        e: Env,
        admin: Address,
        asset: Address,
        treasury: Address,
        property_tax_bps: u32,
    ) -> Result<(), MarketError> {
        if e.storage().instance().has(&DataKey::Admin) {
            return Err(MarketError::AlreadyInitialized);
        }
        admin.require_auth();

        e.storage().instance().set(&DataKey::Admin, &admin);
        e.storage().instance().set(&DataKey::Asset, &asset);
        e.storage().instance().set(&DataKey::Treasury, &treasury);
        e.storage().instance().set(&DataKey::PropertyTaxBps, &property_tax_bps);
        e.storage().instance().set(&DataKey::PropertyCount, &0u32);
        e.storage().instance().extend_ttl(INSTANCE_THRESHOLD, INSTANCE_BUMP);
        Ok(())
    }

    pub fn register_property(
        e: Env,
        owner: Address,
        property_type: PropertyType,
        name: String,
        price_stroops: i128,
        yield_bps: u32,
    ) -> Result<u32, MarketError> {
        owner.require_auth();
        if price_stroops <= 0 {
            return Err(MarketError::InvalidPrice);
        }

        let mut count: u32 = e
            .storage()
            .instance()
            .get(&DataKey::PropertyCount)
            .unwrap_or(0);
        count += 1;

        let deed = PropertyDeed {
            property_id: count,
            owner: owner.clone(),
            property_type,
            name,
            price_stroops,
            for_sale: true,
            yield_bps,
            last_tax_ledger: e.ledger().sequence(),
        };

        let key = DataKey::Deed(count);
        e.storage().persistent().set(&key, &deed);
        e.storage().persistent().extend_ttl(&key, INSTANCE_THRESHOLD, INSTANCE_BUMP);
        e.storage().instance().set(&DataKey::PropertyCount, &count);

        e.events().publish(
            (symbol_short!("market"), symbol_short!("register")),
            (count, owner, price_stroops),
        );
        Ok(count)
    }

    pub fn list_property_for_sale(
        e: Env,
        owner: Address,
        property_id: u32,
        new_price_stroops: i128,
        for_sale: bool,
    ) -> Result<(), MarketError> {
        owner.require_auth();
        if new_price_stroops <= 0 {
            return Err(MarketError::InvalidPrice);
        }

        let key = DataKey::Deed(property_id);
        let mut deed: PropertyDeed = e
            .storage()
            .persistent()
            .get(&key)
            .ok_or(MarketError::PropertyNotFound)?;

        if deed.owner != owner {
            return Err(MarketError::UnauthorizedOwner);
        }

        deed.price_stroops = new_price_stroops;
        deed.for_sale = for_sale;
        e.storage().persistent().set(&key, &deed);
        e.storage().persistent().extend_ttl(&key, INSTANCE_THRESHOLD, INSTANCE_BUMP);

        e.events().publish(
            (symbol_short!("market"), symbol_short!("listed")),
            (property_id, owner, new_price_stroops, for_sale),
        );
        Ok(())
    }

    pub fn buy_property(e: Env, buyer: Address, property_id: u32) -> Result<PropertyDeed, MarketError> {
        buyer.require_auth();
        let key = DataKey::Deed(property_id);
        let mut deed: PropertyDeed = e
            .storage()
            .persistent()
            .get(&key)
            .ok_or(MarketError::PropertyNotFound)?;

        if !deed.for_sale {
            return Err(MarketError::PropertyNotForSale);
        }

        let tax_bps: u32 = e
            .storage()
            .instance()
            .get(&DataKey::PropertyTaxBps)
            .unwrap_or(100);
        let tax_amount = (deed.price_stroops * (tax_bps as i128)) / 10_000;
        let seller_proceeds = deed.price_stroops - tax_amount;

        if let Some(asset_addr) = e.storage().instance().get::<DataKey, Address>(&DataKey::Asset) {
            let treasury_addr: Address = e
                .storage()
                .instance()
                .get(&DataKey::Treasury)
                .ok_or(MarketError::NotInitialized)?;
            let token_client = token::Client::new(&e, &asset_addr);
            if seller_proceeds > 0 {
                token_client.transfer(&buyer, &deed.owner, &seller_proceeds);
            }
            if tax_amount > 0 {
                token_client.transfer(&buyer, &treasury_addr, &tax_amount);
            }
        }

        let previous_owner = deed.owner.clone();
        deed.owner = buyer.clone();
        deed.for_sale = false;
        deed.last_tax_ledger = e.ledger().sequence();

        e.storage().persistent().set(&key, &deed);
        e.storage().persistent().extend_ttl(&key, INSTANCE_THRESHOLD, INSTANCE_BUMP);

        e.events().publish(
            (symbol_short!("market"), symbol_short!("deed_tx")),
            (property_id, previous_owner, buyer, deed.price_stroops, tax_amount),
        );
        Ok(deed)
    }

    pub fn get_property(e: Env, property_id: u32) -> Option<PropertyDeed> {
        e.storage().persistent().get(&DataKey::Deed(property_id))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_register_list_and_buy_property() {
        let e = Env::default();
        e.mock_all_auths();

        let contract_id = e.register_contract(None, RealEstateExchangeContract);
        let client = RealEstateExchangeContractClient::new(&e, &contract_id);

        let alice = Address::generate(&e);
        let bob = Address::generate(&e);

        let prop_id = client.register_property(
            &alice,
            &PropertyType::Bakery,
            &String::from_str(&e, "Sunrise Bakery"),
            &50_000_000,
            &500,
        );
        assert_eq!(prop_id, 1);

        client.list_property_for_sale(&alice, &prop_id, &60_000_000, &true);

        let transferred = client.buy_property(&bob, &prop_id);
        assert_eq!(transferred.owner, bob);
        assert_eq!(transferred.for_sale, false);
        assert_eq!(transferred.price_stroops, 60_000_000);
    }
}
