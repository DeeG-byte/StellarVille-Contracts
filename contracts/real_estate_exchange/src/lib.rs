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
    ItemNotFound = 7,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
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
pub struct PropertyRevenuePool {
    pub property_id: u32,
    pub accumulated_revenue: i128,
    pub total_claimed: i128,
    pub last_claim_ledger: u32,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ItemCategory {
    FoodProvision = 0,
    LuxuryStatus = 1,
    CampaignMegaphone = 2,
    ProductivityTool = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogItem {
    pub item_id: u32,
    pub category: ItemCategory,
    pub name: String,
    pub price_stroops: i128,
    pub energy_restore: u32,
    pub clout_bonus: u32,
    pub business_property_id: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Admin,
    Asset,
    Treasury,
    PropertyTaxBps,
    SalesTaxBps,
    PropertyCount,
    Deed(u32),
    ItemCount,
    Item(u32),
    RevenuePool(u32),
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
        e.storage().instance().set(&DataKey::SalesTaxBps, &150u32);
        e.storage().instance().set(&DataKey::PropertyCount, &0u32);
        e.storage().instance().set(&DataKey::ItemCount, &0u32);
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

    /// Add a consumable or status item tied to an owned commercial property
    pub fn add_catalog_item(
        e: Env,
        owner: Address,
        business_property_id: u32,
        category: ItemCategory,
        name: String,
        price_stroops: i128,
        energy_restore: u32,
        clout_bonus: u32,
    ) -> Result<CatalogItem, MarketError> {
        owner.require_auth();
        if price_stroops <= 0 {
            return Err(MarketError::InvalidPrice);
        }

        let deed: PropertyDeed = e
            .storage()
            .persistent()
            .get(&DataKey::Deed(business_property_id))
            .ok_or(MarketError::PropertyNotFound)?;

        if deed.owner != owner {
            return Err(MarketError::UnauthorizedOwner);
        }

        let mut item_count: u32 = e
            .storage()
            .instance()
            .get(&DataKey::ItemCount)
            .unwrap_or(0);
        item_count += 1;

        let item = CatalogItem {
            item_id: item_count,
            category,
            name,
            price_stroops,
            energy_restore,
            clout_bonus,
            business_property_id,
        };

        let key = DataKey::Item(item_count);
        e.storage().persistent().set(&key, &item);
        e.storage().persistent().extend_ttl(&key, INSTANCE_THRESHOLD, INSTANCE_BUMP);
        e.storage().instance().set(&DataKey::ItemCount, &item_count);

        e.events().publish(
            (symbol_short!("market"), symbol_short!("item_add")),
            (item_count, business_property_id, price_stroops),
        );
        Ok(item)
    }

    /// Purchase a catalog item, routing sales tax to treasury and net revenue to property owner
    pub fn buy_item(
        e: Env,
        buyer: Address,
        item_id: u32,
        quantity: u32,
    ) -> Result<(i128, i128), MarketError> {
        buyer.require_auth();
        if quantity == 0 {
            return Err(MarketError::InvalidPrice);
        }

        let item: CatalogItem = e
            .storage()
            .persistent()
            .get(&DataKey::Item(item_id))
            .ok_or(MarketError::ItemNotFound)?;

        let deed: PropertyDeed = e
            .storage()
            .persistent()
            .get(&DataKey::Deed(item.business_property_id))
            .ok_or(MarketError::PropertyNotFound)?;

        let total_cost = item.price_stroops * (quantity as i128);
        let sales_tax_bps: u32 = e
            .storage()
            .instance()
            .get(&DataKey::SalesTaxBps)
            .unwrap_or(150);
        let tax_amount = (total_cost * (sales_tax_bps as i128)) / 10_000;
        let owner_revenue = total_cost - tax_amount;

        if let Some(asset_addr) = e.storage().instance().get::<DataKey, Address>(&DataKey::Asset) {
            let treasury_addr: Address = e
                .storage()
                .instance()
                .get(&DataKey::Treasury)
                .ok_or(MarketError::NotInitialized)?;
            let token_client = token::Client::new(&e, &asset_addr);
            if owner_revenue > 0 {
                token_client.transfer(&buyer, &deed.owner, &owner_revenue);
            }
            if tax_amount > 0 {
                token_client.transfer(&buyer, &treasury_addr, &tax_amount);
            }
        }

        let pool_key = DataKey::RevenuePool(item.business_property_id);
        let mut pool: PropertyRevenuePool = e
            .storage()
            .persistent()
            .get(&pool_key)
            .unwrap_or(PropertyRevenuePool {
                property_id: item.business_property_id,
                accumulated_revenue: 0,
                total_claimed: 0,
                last_claim_ledger: e.ledger().sequence(),
            });
        pool.accumulated_revenue += owner_revenue;
        e.storage().persistent().set(&pool_key, &pool);

        e.events().publish(
            (symbol_short!("market"), symbol_short!("item_buy")),
            (item_id, buyer, quantity, owner_revenue, tax_amount),
        );
        Ok((owner_revenue, tax_amount))
    }

    pub fn get_item(e: Env, item_id: u32) -> Option<CatalogItem> {
        e.storage().persistent().get(&DataKey::Item(item_id))
    }

    pub fn get_property(e: Env, property_id: u32) -> Option<PropertyDeed> {
        e.storage().persistent().get(&DataKey::Deed(property_id))
    }

    pub fn claim_property_yield(
        e: Env,
        owner: Address,
        property_id: u32,
    ) -> Result<i128, MarketError> {
        owner.require_auth();
        let deed: PropertyDeed = e
            .storage()
            .persistent()
            .get(&DataKey::Deed(property_id))
            .ok_or(MarketError::PropertyNotFound)?;
        if deed.owner != owner {
            return Err(MarketError::UnauthorizedOwner);
        }
        let pool_key = DataKey::RevenuePool(property_id);
        let mut pool: PropertyRevenuePool = e
            .storage()
            .persistent()
            .get(&pool_key)
            .unwrap_or(PropertyRevenuePool {
                property_id,
                accumulated_revenue: 0,
                total_claimed: 0,
                last_claim_ledger: e.ledger().sequence(),
            });
        let claimable = pool.accumulated_revenue;
        pool.accumulated_revenue = 0;
        pool.total_claimed += claimable;
        pool.last_claim_ledger = e.ledger().sequence();
        e.storage().persistent().set(&pool_key, &pool);

        e.events().publish(
            (symbol_short!("market"), symbol_short!("claim_yd")),
            (property_id, owner, claimable),
        );
        Ok(claimable)
    }

    pub fn get_revenue_pool(e: Env, property_id: u32) -> PropertyRevenuePool {
        e.storage()
            .persistent()
            .get(&DataKey::RevenuePool(property_id))
            .unwrap_or(PropertyRevenuePool {
                property_id,
                accumulated_revenue: 0,
                total_claimed: 0,
                last_claim_ledger: e.ledger().sequence(),
            })
    }

}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_catalog_item_inventory_and_purchase() {
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

        let item = client.add_catalog_item(
            &alice,
            &prop_id,
            &ItemCategory::FoodProvision,
            &String::from_str(&e, "Artisan Grain Loaf"),
            &10_000,
            &25,
            &5,
        );
        assert_eq!(item.item_id, 1);

        let (owner_rev, tax) = client.buy_item(&bob, &1, &2);
        // 2 * 10,000 = 20,000 total; 1.5% sales tax = 300; owner revenue = 19,700
        assert_eq!(owner_rev, 19_700);
        assert_eq!(tax, 300);
    }

    #[test]
    fn test_commercial_property_passive_yield_distribution() {
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

        client.add_catalog_item(
            &alice,
            &prop_id,
            &ItemCategory::FoodProvision,
            &String::from_str(&e, "Artisan Bread"),
            &10_000,
            &25,
            &5,
        );

        client.buy_item(&bob, &1, &2);
        let pool = client.get_revenue_pool(&prop_id);
        assert_eq!(pool.accumulated_revenue, 19_700);
        assert_eq!(pool.total_claimed, 0);

        let claimed = client.claim_property_yield(&alice, &prop_id);
        assert_eq!(claimed, 19_700);

        let pool_after = client.get_revenue_pool(&prop_id);
        assert_eq!(pool_after.accumulated_revenue, 0);
        assert_eq!(pool_after.total_claimed, 19_700);
    }

}
