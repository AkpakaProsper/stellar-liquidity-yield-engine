//! Oracle Registry — centralised price-feed management for the yield engine.
//!
//! **Issue #281** – per-pool freshness thresholds: each pool can declare its
//! own maximum age (in seconds) for a price observation before it is
//! considered stale.
//!
//! **Issue #288** – bounded rolling window of recent observations: each feed
//! keeps at most `MAX_OBSERVATIONS` entries; the oldest is evicted when the
//! window is full.

#![allow(dead_code)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, Address, Env, Map,
    Symbol, Vec,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of price observations stored per feed (Issue #288).
pub const MAX_OBSERVATIONS: u32 = 10;

/// Default freshness threshold in seconds (5 minutes).
pub const DEFAULT_FRESHNESS_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[contracterror]
#[derive(Clone, Debug, Eq, PartialEq, Copy)]
pub enum OracleError {
    /// The caller is not the contract admin.
    Unauthorized = 1,
    /// The requested feed does not exist.
    FeedNotFound = 2,
    /// A feed for the given pool already exists.
    FeedAlreadyExists = 3,
    /// The caller is not the authorised feeder for this feed.
    NotAuthorisedFeeder = 4,
    /// No fresh observation is available.
    NoFreshObservation = 5,
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single price observation stored in the rolling window (Issue #288).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceObservation {
    /// Price in the smallest denomination of the quote asset.
    pub price: i128,
    /// Ledger timestamp at which this observation was recorded.
    pub timestamp: u64,
}

/// Configuration and state for one oracle feed (keyed by pool address).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleFeed {
    /// Pool this feed supplies price data for.
    pub pool_address: Address,
    /// The single keeper address authorised to publish observations.
    pub feeder: Address,
    /// Maximum age (seconds) before an observation is considered stale (Issue #281).
    pub freshness_threshold_secs: u64,
    /// Bounded rolling window of recent observations (Issue #288).
    pub observations: Vec<PriceObservation>,
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

fn feeds_key(env: &Env) -> Symbol {
    Symbol::new(env, "oracle_feeds")
}

fn admin_key(env: &Env) -> Symbol {
    Symbol::new(env, "oracle_admin")
}

fn load_feeds(env: &Env) -> Map<Address, OracleFeed> {
    env.storage()
        .instance()
        .get(&feeds_key(env))
        .unwrap_or(Map::new(env))
}

fn save_feeds(env: &Env, feeds: &Map<Address, OracleFeed>) {
    env.storage().instance().set(&feeds_key(env), feeds);
}

fn load_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&admin_key(env))
        .expect("oracle registry not initialised")
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct OracleRegistry;

#[contractimpl]
impl OracleRegistry {
    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Initialise the oracle registry.  Must be called exactly once.
    pub fn initialize(env: Env, admin: Address) {
        env.storage().instance().set(&admin_key(&env), &admin);
        let feeds: Map<Address, OracleFeed> = Map::new(&env);
        save_feeds(&env, &feeds);
    }

    // -----------------------------------------------------------------------
    // Feed management (admin)
    // -----------------------------------------------------------------------

    /// Register a new oracle feed for `pool_address`.
    ///
    /// Pass `0` for `freshness_threshold_secs` to use the contract default (300 s).
    pub fn register_feed(
        env: Env,
        admin: Address,
        pool_address: Address,
        feeder: Address,
        freshness_threshold_secs: u64,
    ) {
        Self::check_admin(&env, &admin);

        let mut feeds = load_feeds(&env);
        if feeds.contains_key(pool_address.clone()) {
            panic_with_error!(&env, OracleError::FeedAlreadyExists);
        }

        let threshold = if freshness_threshold_secs == 0 {
            DEFAULT_FRESHNESS_SECS
        } else {
            freshness_threshold_secs
        };

        let feed = OracleFeed {
            pool_address: pool_address.clone(),
            feeder,
            freshness_threshold_secs: threshold,
            observations: Vec::new(&env),
        };

        feeds.set(pool_address.clone(), feed);
        save_feeds(&env, &feeds);

        env.events()
            .publish(("oracle_feed_registered",), (&pool_address,));
    }

    // -----------------------------------------------------------------------
    // Issue #281 – per-pool freshness threshold
    // -----------------------------------------------------------------------

    /// Update the staleness threshold for a specific pool's feed (admin only).
    pub fn set_freshness_threshold(
        env: Env,
        admin: Address,
        pool_address: Address,
        freshness_threshold_secs: u64,
    ) {
        Self::check_admin(&env, &admin);

        let mut feeds = load_feeds(&env);
        let mut feed = match feeds.get(pool_address.clone()) {
            Some(f) => f,
            None => panic_with_error!(&env, OracleError::FeedNotFound),
        };

        feed.freshness_threshold_secs = freshness_threshold_secs;
        feeds.set(pool_address, feed);
        save_feeds(&env, &feeds);
    }

    /// Return the freshness threshold for a pool.
    pub fn get_freshness_threshold(env: Env, pool_address: Address) -> u64 {
        let feeds = load_feeds(&env);
        match feeds.get(pool_address) {
            Some(f) => f.freshness_threshold_secs,
            None => panic_with_error!(&env, OracleError::FeedNotFound),
        }
    }

    // -----------------------------------------------------------------------
    // Issue #288 – bounded rolling window + price publishing
    // -----------------------------------------------------------------------

    /// Publish a new price observation for a feed.
    ///
    /// Observations are appended to the rolling window; when it reaches
    /// `MAX_OBSERVATIONS` the oldest entry is dropped (Issue #288).
    pub fn publish_price(env: Env, feeder: Address, pool_address: Address, price: i128) {
        let mut feeds = load_feeds(&env);
        let mut feed = match feeds.get(pool_address.clone()) {
            Some(f) => f,
            None => panic_with_error!(&env, OracleError::FeedNotFound),
        };

        if feed.feeder != feeder {
            panic_with_error!(&env, OracleError::NotAuthorisedFeeder);
        }

        let obs = PriceObservation {
            price,
            timestamp: env.ledger().timestamp(),
        };

        // Evict oldest observation when window is full (Issue #288).
        if feed.observations.len() >= MAX_OBSERVATIONS {
            let mut trimmed: Vec<PriceObservation> = Vec::new(&env);
            let len = feed.observations.len();
            for i in 1..len {
                trimmed.push_back(feed.observations.get(i).unwrap());
            }
            feed.observations = trimmed;
        }

        feed.observations.push_back(obs);
        feeds.set(pool_address.clone(), feed);
        save_feeds(&env, &feeds);

        env.events()
            .publish(("oracle_price_published",), (&pool_address, price));
    }

    /// Return the observation window for a feed (Issue #288).
    pub fn get_observations(env: Env, pool_address: Address) -> Vec<PriceObservation> {
        let feeds = load_feeds(&env);
        match feeds.get(pool_address) {
            Some(f) => f.observations,
            None => panic_with_error!(&env, OracleError::FeedNotFound),
        }
    }

    /// Return the latest fresh price for `pool_address` (Issue #281).
    pub fn get_price(env: Env, pool_address: Address) -> i128 {
        let now = env.ledger().timestamp();
        let feeds = load_feeds(&env);

        let feed = match feeds.get(pool_address.clone()) {
            Some(f) => f,
            None => panic_with_error!(&env, OracleError::FeedNotFound),
        };

        // Walk backwards — newest observation is at the end of the Vec.
        let len = feed.observations.len();
        for i in (0..len).rev() {
            let obs = feed.observations.get(i).unwrap();
            let age = now.saturating_sub(obs.timestamp);
            if age <= feed.freshness_threshold_secs {
                return obs.price;
            }
        }

        panic_with_error!(&env, OracleError::NoFreshObservation)
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    fn check_admin(env: &Env, caller: &Address) {
        let admin = load_admin(env);
        if *caller != admin {
            panic_with_error!(env, OracleError::Unauthorized);
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger, LedgerInfo};
    use soroban_sdk::Env;

    fn set_timestamp(env: &Env, ts: u64) {
        env.ledger().set(LedgerInfo {
            timestamp: ts,
            protocol_version: 22,
            sequence_number: ts as u32,
            network_id: Default::default(),
            base_reserve: 10,
            min_temp_entry_ttl: 1,
            min_persistent_entry_ttl: 1,
            max_entry_ttl: 999_999,
        });
    }

    fn deploy(env: &Env) -> (Address, OracleRegistryClient) {
        let admin = Address::generate(env);
        let contract_id = env.register_contract(None, OracleRegistry);
        let client = OracleRegistryClient::new(env, &contract_id);
        client.initialize(&admin);
        (admin, client)
    }

    // ------------------------------------------------------------------
    // Issue #288 – bounded rolling window
    // ------------------------------------------------------------------

    /// After MAX_OBSERVATIONS entries the oldest is dropped.
    #[test]
    fn test_rolling_window_evicts_oldest() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, client) = deploy(&env);

        let feeder = Address::generate(&env);
        let pool = Address::generate(&env);

        client.register_feed(&admin, &pool, &feeder, &DEFAULT_FRESHNESS_SECS);

        // Publish MAX_OBSERVATIONS + 1 entries.
        for i in 0..(MAX_OBSERVATIONS + 1) {
            set_timestamp(&env, 1_000 + i as u64 * 10);
            client.publish_price(&feeder, &pool, &(i as i128));
        }

        let obs = client.get_observations(&pool);

        // Window must not exceed MAX_OBSERVATIONS.
        assert_eq!(obs.len(), MAX_OBSERVATIONS);

        // The first entry (price=0) should have been evicted.
        assert_eq!(obs.get(0).unwrap().price, 1);

        // The last entry is the most recently published.
        assert_eq!(obs.get(MAX_OBSERVATIONS - 1).unwrap().price, MAX_OBSERVATIONS as i128);
    }

    /// Exactly MAX_OBSERVATIONS entries fit without eviction.
    #[test]
    fn test_rolling_window_exact_capacity() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, client) = deploy(&env);

        let feeder = Address::generate(&env);
        let pool = Address::generate(&env);

        client.register_feed(&admin, &pool, &feeder, &DEFAULT_FRESHNESS_SECS);

        for i in 0..MAX_OBSERVATIONS {
            set_timestamp(&env, 1_000 + i as u64 * 10);
            client.publish_price(&feeder, &pool, &(i as i128));
        }

        let obs = client.get_observations(&pool);
        assert_eq!(obs.len(), MAX_OBSERVATIONS);
        assert_eq!(obs.get(0).unwrap().price, 0);
        assert_eq!(obs.get(MAX_OBSERVATIONS - 1).unwrap().price, (MAX_OBSERVATIONS - 1) as i128);
    }

    /// An empty window returns NoFreshObservation.
    #[test]
    fn test_empty_feed_returns_no_observation() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, client) = deploy(&env);

        let pool = Address::generate(&env);
        let feeder = Address::generate(&env);

        client.register_feed(&admin, &pool, &feeder, &DEFAULT_FRESHNESS_SECS);

        set_timestamp(&env, 1000);
        let err = client.try_get_price(&pool).unwrap_err().unwrap();
        assert_eq!(
            err,
            soroban_sdk::Error::from_contract_error(OracleError::NoFreshObservation as u32)
        );
    }

    // ------------------------------------------------------------------
    // Issue #281 – per-pool freshness thresholds (regression)
    // ------------------------------------------------------------------

    /// Fresh price is returned; stale price triggers NoFreshObservation.
    #[test]
    fn test_per_pool_freshness_threshold() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, client) = deploy(&env);

        let feeder = Address::generate(&env);
        let pool_relaxed = Address::generate(&env);
        let pool_strict = Address::generate(&env);

        client.register_feed(&admin, &pool_relaxed, &feeder, &600_u64);
        client.register_feed(&admin, &pool_strict, &feeder, &30_u64);

        set_timestamp(&env, 1000);
        client.publish_price(&feeder, &pool_relaxed, &500_i128);
        client.publish_price(&feeder, &pool_strict, &500_i128);

        set_timestamp(&env, 1200);

        assert_eq!(client.get_price(&pool_relaxed), 500);

        let err = client.try_get_price(&pool_strict).unwrap_err().unwrap();
        assert_eq!(
            err,
            soroban_sdk::Error::from_contract_error(OracleError::NoFreshObservation as u32)
        );
    }
}
