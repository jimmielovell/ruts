# Changelog

## [0.5.4] - 2024-02-16

### Changed
- `prepare_regenerate()` will rename a session id if it exists, if not, a new session id is set instead.

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
