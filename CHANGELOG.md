# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.11.1] - Unreleased

### Fixed

- Items that only one feature uses are now compiled out with it instead of built
  and warned about: `MappingId` and the mapping cell on `Id` (`scylla-store`),
  `Session::new` and the extractor's fields on `Inner` (`axum`, `signed`),
  `SessionMap::new` (any backend store), and Scylla's all-fields-with-meta
  statement (`layered-store`).
- The `axum` test target declares its `required-features`; it was built in
  configurations without `moka-store`, where it could not compile.

### Performance

- Without `scylla-store`, `Id` no longer carries or allocates an
  `Arc<RwLock<..>>` for the mapping cell.
- Without `layered-store`, `ScyllaStore` no longer prepares a statement at
  startup that nothing can execute.

## [0.11.0] - 2026-09-23

### Added

- `ScyllaStore` and `ScyllaStoreBuilder`, behind the new `scylla-store` feature.
- `MokaStore` and `MokaStoreBuilder`, behind the new `moka-store` feature.
- `LayeredStore` can now pair a hot store with `ScyllaStore` as the cold tier.
- `Ttl::ZERO`: writing a field with it removes the field instead of storing it,
  and never leaves the old value behind. `Ttl::is_zero` tests for it.
- A `hot_cache_ttl` of `Ttl::ZERO` keeps a field out of the hot store entirely,
  persisted and read from the cold store, never cached, including on warming.
- `Session::expire_field`, to extend one field's TTL.
- `Session::cookie_max_age`, to read the cookie lifetime in effect.
- A conformance suite every backend runs, plus integration coverage for
  `LayeredStore`, which had none.

### Breaking

- TTLs are now a validated `Ttl` (`0..=i32::MAX` seconds) instead of `Option<i64>`
  seconds, and there is no "never expires" any more.
- `Session::set` returns `Result<()>`; `SessionStore` methods return `Result<()>`
  or `Result<bool>` rather than the session TTL as `Result<i64>`.
- `SessionStore::remove` returns `Result<bool>`. `Session::remove` still returns
  `Result<bool>`, but it now means "the field was there to remove" rather than
  "the session still exists".
- `Session::expire` is gone: use `expire_field` for a field, `set_expiration` for
  the cookie. `set_expiration` takes a `u64`.
- `Id` is no longer `Copy`. It carries a write-once mapping id behind a shared
  cell so a store can resolve a presented id once; it stays `Clone`, and `&Id`
  call sites are unaffected.
- `Id` is stored as 22 base64 bytes instead of 16 raw bytes, which breaks
  `bincode` round-trips of a serialized `Id`.
- `layered-store` no longer pulls in `redis-store` and `postgres-store`; declare
  the combination you want.
- `MemoryStore` is removed in favour of `MokaStore`.
- `messagepack` takes precedence when both serialization features are enabled.

### Performance

- `Id::as_str` returns a slice without allocating.
- Scylla rotations re-point a single mapping row instead of rewriting every
  field, and batch the accompanying write into the same round trip.

### Fixed

- `LayeredStore::get` and `get_all` no longer panic when the cold store reports a
  field without cache metadata; cold stores now report metadata for every field
  they return, a field in its last second included.
- `ScyllaStore` with a zero TTL made a field permanent.
- `MokaStore::set_and_rename` with a zero TTL left a dead entry behind that kept
  the session alive and made a later rename onto that id fail as a collision.
- `PostgresStore` with a zero TTL wrote an already-expired row instead of
  deleting, and `remove` reported a lapsed row as removed.
- `postgres-store` builds without `layered-store`.

## [0.10.0] - 2026-05-18

### Changed
- **Breaking:** `RedisStore::new` is now `async` and returns `Result<Self, Error>`.
  Lua scripts are pre-loaded at construction instead of cached per-call.
- **Breaking:** `SessionStore::set` and `SessionStore::set_and_rename` no longer
  require `T: 'static`.
- **Breaking:** `PostgresStoreBuilder::new` no longer takes `create_table` as a
    positional argument. Use the `create_table(bool)` builder method instead;
    defaults to `false`.
- **Breaking:** `PostgresStoreBuilder::table_name` and `schema_name` now return
  `Result` and reject identifiers outside `[A-Za-z_][A-Za-z0-9_]{0,62}`.
- `PostgresStore` cleanup task now runs expired-session and expired-field
  deletes in a single statement, and is aborted when the original store is
  dropped.
- `MemoryStore::set_and_rename` now returns an error when the target session
    already exists, matching the redis and postgres stores.
- `MemoryStore::expire` no longer extends field TTLs beyond their original
  expiry; behavior now matches the postgres store.

### Added
- `RedisStore::reload_scripts` for manual re-loading after server-side
  cache loss (restart, failover, `SCRIPT FLUSH`).

### Fixed
- `PostgresStore::set_and_rename` with `field_ttl_secs == 0` no longer leaves
  the store in an inconsistent state on crash; rename and remove now run in
  a single transaction with the correct ordering.
- `PostgresStore::expire(sid, -1)` now correctly persists field expiries
  along with the session, instead of leaving fields with their original TTLs.
- `PostgresStore` table and schema names are now validated to prevent SQL
  injection via embedded quotes.

## [0.9.0] - 2026-03-06

### Added
- Support for cryptographically signed cookies via the new optional `signed` feature.

### Changed
- Removed `derive(Debug)` from the `Id` type to prevent accidental logging of sensitive session IDs.

### Removed
- Removed public re-exports of `fred` and `sqlx` crates.

## [0.8.1] - 2026-01-10
### Changed
- **Layered Store:** `Session::get_all` should always fetch from the cold store as the hot store could be missing some field/value(s).


## [0.8.0] - 2026-01-07

### Breaking Changes
- Updated `SessionStore` trait methods (`set` and `set_and_rename`) to use `i64` for `key_ttl` and `field_ttl` instead of `Option<i64>`.
  - `> 0`: Finite TTL (seconds).
  - `0`: Delete.
  - `-1`: Persistent.
- Updated `LayeredColdStore` trait methods (`set_with_meta` and `set_and_rename_with_meta`) to match the new `i64` signature.

### Changed
- Moved the "Effective TTL" calculation logic (resolving defaults and max(session, field) rules) from individual store backends into the `Session` struct. This ensures consistent expiration behavior across Memory, Redis, and Postgres stores.
- **Redis:** Simplified Lua scripts (`SET`, `SET_AND_RENAME`) by removing internal TTL derivation logic.

## [0.7.3] - 2025-12-29

### Fixed
- **Postgres:** Fixed a race condition in `set_and_rename` where the concurrent execution of the rename (UPDATE) and upsert (INSERT) CTEs caused "duplicate key value" violations.

## [0.7.2] - 2025-12-29

### Breaking Changes
- **API:** Consolidated `insert` and `update` methods into a single `set` method (upsert semantics) in the `SessionStore` trait.
- **API:** Consolidated `insert_with_rename` and `update_with_rename` into `set_and_rename`.
- **Postgres:** Changed schema to a normalized two-table design (`sessions` for lifecycle and `*_kv` for data) to accurately handle per-field expiration.

### Changed
- **Postgres:** Implemented single-round-trip CTEs for all operations to ensure atomicity, handle `ON UPDATE CASCADE` natively, and reduce network overhead.
- **Redis:** Rewrote Lua scripts to enforce strict session fixation protection (abort on rename collision) and consistent TTL extension logic.
- **Core:** Session expiry now strictly relies on the database server's time (Postgres `now()`, Redis `TTL`) rather than application time to eliminate clock skew issues.

## [0.7.0] - 2025-09-23

### Breaking Changes

- Simplified LayeredStore API: The SessionStore trait has been updated to provide a more direct and ergonomic API for the LayeredStore.
- The insert, update, insert_with_rename, and update_with_rename methods now include an optional hot_cache_ttl_secs parameter.

## [0.6.4] - 2025-09-12

### Fixed

- Corrected session TTL handling.

## [0.6.1] - 2025-09-06

### Fixed

- Fixed a bug in `LayeredStore` where warming up the hot cache would ignore the original write strategy.

### Breaking Changes

- `redis-store` is no longer enabled by default. One has to explicitly enable it in `Cargo.toml`.

## [0.6.0] - 2025-09-05

### Added

- `PostgresStore`, a new session store backend for PostgreSQL, available under the postgres-store feature flag.
- `LayeredStore` that layers a fast, ephemeral "hot" cache (like Redis) on top of a slower, persistent "cold" store (like Postgres).

### Changed

- The `get_all` method now returns an `Option<SessionMap>`, a wrapper around a `DashMap<String, Vec<u8>>`. This allows for efficient bulk fetching of all session data with lazy, on-demand deserialization of individual fields, avoiding issues with non-self-describing serialization formats.
- The internal implementation of `MemoryStore` now uses `dashmap::DashMap`.

### Breaking Changes

- `Session` and `SessionService` no longer default to using `RedisStore`. Users must now explicitly specify the store type they are using (e.g., `Session<RedisStore>`, `SessionLayer::new<PostgresStore>(...))`.

## [0.5.9] - 2025-08-11

### Added

- Support for bincode serialization backend

### Changed

- BREAKING: Default serialization backend changed from messagepack to bincode
- redis-store feature no longer automatically enables messagepack
- Improved documentation with serialization feature examples

## [0.5.8] - 2025-07-07

- Optimized update/insert redis lua scripts
- Use Ordering::SeqCst for all cookie max_age operations

## [0.5.6] - 2024-02-16

### Fixed

- Update/insert redis session with rename even if old_key is not found in store

## [0.5.5] - 2024-02-16

### Fixed

- Inconsistent session expiration behavior in Redis UPDATE script
- Race conditions and atomicity issues in Redis UPDATE_WITH_RENAME script

## [0.5.4] - 2024-02-16

### Changed
- `prepare_regenerate()` will rename a session id if it exists, if not, a new session id is set instead.

## [0.5.3] - 2024-02-08

### Added
- New `prepare_regenerate()` method for atomic session ID regeneration with update/insert
- Support for atomic (insert/update with ID regeneration) operations in both Redis and Memory stores

## [0.5.2] - 2024-02-07

### Added
- implement `Clone` for `Session`

## [0.5.1] - 2024-02-06

### Added
- Field-level expiration support using Redis HEXPIRE command
- Support for optional field expiration in hash entries
- Lua scripts for atomic operations to improve performance thereby reducing Redis network calls by 50% for these operations:
  - Combined HSETNX/HSET with EXPIRE and HEXPIRE into a single round-trip
  - Combined RENAMENX with EXPIRE into a single round-trip

### Changed
- Minimum Redis version requirement is now 7.4 due to HEXPIRE command usage

### Notes
- Users with Redis versions < 7.4 will need to handle field expiration differently or upgrade their Redis instance

## [0.5.0] - 2024-01-11

### Breaking Changes
- Migrated to native async traits with the following changes:
  - Removed `#[async_trait]` attribute from `FromRequestParts` implementation to support Axum 0.8+ compatibility
  - Refactored `SessionStore` trait to use explicit `Future` returns instead of `async fn`
  - If you're using Axum < 0.8, please continue using ruts version 0.4.3

### Added
- Support for Axum 0.8+

### Dependencies
- Updated minimum supported Axum version to 0.8.0

### Migration Guide
If you're upgrading to Axum 0.8+ and using ruts:
1. Update your Axum dependency to 0.8.0 or higher
2. Update ruts to the latest version
3. No additional code changes are required for session handling

The session middleware and extractors will continue to work as before:
```rust
// Your code will continue to work unchanged
async fn handler(
    session: Session<RedisStore<Pool>>,
    // ... other parameters
) -> Result<(), Error> {
    // ... your code
}
```

## [0.4.2] - 2024-12-14
### Fixed
- Match Cargo.toml version and install version in the README.md

## [0.4.0] - 2024-12-14
### Added
- New `MemoryStore` implementation for development and testing environments
- Comprehensive test suite for redis and axum session extraction

## [0.4.0] - 2024-12-14
### Added
- New `MemoryStore` implementation for development and testing environments
- Comprehensive test suite for redis and axum session extraction

### Changed
- Improved session state management by combining state flags into a single `AtomicU8`
- Replaced `Mutex<Option<Cookies>>` with `OnceLock<Cookies>` for better performance
- Updated internal session management to use more efficient state handling

# [0.3.0] - 2024-10-26
### Added
- `set_expiration` to enable setting a different expiration from the one set in CookieOptions.

### Fixed
- `session.get_all` now returns a `Deserialize`able type `T` instead of `Hashmap<String, T>`

# [0.2.0] - 2024-10-17

### Changed
- Specify `axum` and `redis-store` as optional features

### Removed
- Removed `insert_multiple`

# [0.1.11] - 2024-10-12
- Initial Release