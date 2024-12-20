# Changelog

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
