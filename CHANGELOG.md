# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
