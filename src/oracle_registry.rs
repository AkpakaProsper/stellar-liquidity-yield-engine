//! Oracle Registry — centralised price-feed management for the yield engine.
//!
//! **Issue #281** – per-pool freshness thresholds: each pool can declare its
//! own maximum age (in seconds) for a price observation before it is
//! considered stale.

#![allow(dead_code)]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, Address, Env, Map,
    Symbol,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

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

/// Configuration and state for one oracle feed (keyed by pool address).
///
/// **Issue #281**: `freshness_threshold_secs` is per-pool so different pools
/// can tolerate different data ages.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleFeed {
    /// Pool this feed supplies price data for.
    pub pool_address: Address,
    /// The single keeper address authorised to publish observations.
    pub feeder: Address,
    /// Maximum age (seconds) before an observation is considered stale.
    /// Each pool may have a different value (Issue #281).
    pub freshness_threshold_secs: u64,
    /// Whether a price has been published yet.
    pub has_latest: bool,
    /// Most recently published price (valid only when `has_latest` is true).
    pub latest_price: i128,
    /// Timestamp of the most recent price (valid only when `has_latest` is true).
    pub latest_timestamp: u64,
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
    /// `freshness_threshold_secs` sets the per-pool staleness window (Issue #281).
    /// Pass `0` to use the contract default (300 s).
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
            has_latest: false,
            latest_price: 0,
            latest_timestamp: 0,
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
    // Price publishing & querying
    // -----------------------------------------------------------------------

    /// Publish a new price observation for a feed (authorised feeder only).
    pub fn publish_price(env: Env, feeder: Address, pool_address: Address, price: i128) {
        let mut feeds = load_feeds(&env);
        let mut feed = match feeds.get(pool_address.clone()) {
            Some(f) => f,
            None => panic_with_error!(&env, OracleError::FeedNotFound),
        };

        if feed.feeder != feeder {
            panic_with_error!(&env, OracleError::NotAuthorisedFeeder);
        }

        feed.has_latest = true;
        feed.latest_price = price;
        feed.latest_timestamp = env.ledger().timestamp();

        feeds.set(pool_address.clone(), feed);
        save_feeds(&env, &feeds);

        env.events()
            .publish(("oracle_price_published",), (&pool_address, price));
    }

    /// Return the latest fresh price for `pool_address`.
    ///
    /// Panics with `NoFreshObservation` if the latest observation is older
    /// than the pool's `freshness_threshold_secs` (Issue #281) or if no
    /// observation has been published yet.
    pub fn get_price(env: Env, pool_address: Address) -> i128 {
        let now = env.ledger().timestamp();
        let feeds = load_feeds(&env);

        let feed = match feeds.get(pool_address.clone()) {
            Some(f) => f,
            None => panic_with_error!(&env, OracleError::FeedNotFound),
        };

        if !feed.has_latest {
            panic_with_error!(&env, OracleError::NoFreshObservation);
        }

        let age = now.saturating_sub(feed.latest_timestamp);
        if age <= feed.freshness_threshold_secs {
            feed.latest_price
        } else {
            panic_with_error!(&env, OracleError::NoFreshObservation)
        }
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

    /// A pool with a generous threshold accepts an older observation; a pool
    /// with a tight threshold rejects the same observation.
    #[test]
    fn test_per_pool_freshness_threshold() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, client) = deploy(&env);

        let feeder = Address::generate(&env);
        let pool_relaxed = Address::generate(&env);
        let pool_strict = Address::generate(&env);

        // Relaxed: 600 s threshold.
        client.register_feed(&admin, &pool_relaxed, &feeder, &600_u64);
        // Strict: 30 s threshold.
        client.register_feed(&admin, &pool_strict, &feeder, &30_u64);

        // Publish at t=1000.
        set_timestamp(&env, 1000);
        client.publish_price(&feeder, &pool_relaxed, &500_i128);
        client.publish_price(&feeder, &pool_strict, &500_i128);

        // Query at t=1200 (200 s after publication).
        set_timestamp(&env, 1200);

        // Relaxed pool still fresh (200 < 600).
        assert_eq!(client.get_price(&pool_relaxed), 500);

        // Strict pool stale (200 > 30).
        let err = client.try_get_price(&pool_strict).unwrap_err().unwrap();
        assert_eq!(
            err,
            soroban_sdk::Error::from_contract_error(OracleError::NoFreshObservation as u32)
        );
    }

    /// Admin can update the threshold after registration.
    #[test]
    fn test_update_freshness_threshold() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, client) = deploy(&env);

        let feeder = Address::generate(&env);
        let pool = Address::generate(&env);

        client.register_feed(&admin, &pool, &feeder, &60_u64);

        set_timestamp(&env, 1000);
        client.publish_price(&feeder, &pool, &1_000_i128);

        // 120 s later – stale under the 60 s threshold.
        set_timestamp(&env, 1120);
        let err = client.try_get_price(&pool).unwrap_err().unwrap();
        assert_eq!(
            err,
            soroban_sdk::Error::from_contract_error(OracleError::NoFreshObservation as u32)
        );

        // Relax threshold to 300 s.
        client.set_freshness_threshold(&admin, &pool, &300_u64);
        assert_eq!(client.get_freshness_threshold(&pool), 300);
        assert_eq!(client.get_price(&pool), 1_000);
    }

    /// Default freshness threshold is applied when 0 is passed.
    #[test]
    fn test_default_threshold_applied_on_zero() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, client) = deploy(&env);

        let feeder = Address::generate(&env);
        let pool = Address::generate(&env);

        client.register_feed(&admin, &pool, &feeder, &0_u64);
        assert_eq!(client.get_freshness_threshold(&pool), DEFAULT_FRESHNESS_SECS);
    }

    /// Only admin can register a feed.
    #[test]
    fn test_non_admin_cannot_register_feed() {
        let env = Env::default();
        env.mock_all_auths();
        let (_admin, client) = deploy(&env);

        let imposter = Address::generate(&env);
        let feeder = Address::generate(&env);
        let pool = Address::generate(&env);

        let err = client
            .try_register_feed(&imposter, &pool, &feeder, &DEFAULT_FRESHNESS_SECS)
            .unwrap_err()
            .unwrap();
        assert_eq!(
            err,
            soroban_sdk::Error::from_contract_error(OracleError::Unauthorized as u32)
        );
    }

    /// Unauthorised feeder is rejected.
    #[test]
    fn test_unauthorised_feeder_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (admin, client) = deploy(&env);

        let feeder = Address::generate(&env);
        let stranger = Address::generate(&env);
        let pool = Address::generate(&env);

        client.register_feed(&admin, &pool, &feeder, &DEFAULT_FRESHNESS_SECS);

        set_timestamp(&env, 1000);
        let err = client
            .try_publish_price(&stranger, &pool, &100_i128)
            .unwrap_err()
            .unwrap();
        assert_eq!(
            err,
            soroban_sdk::Error::from_contract_error(OracleError::NotAuthorisedFeeder as u32)
        );
    }
}
