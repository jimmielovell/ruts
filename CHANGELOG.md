## [Unreleased]
### Changed
- Improved session state management by combining state flags into a single `AtomicU8`
- Replaced `Mutex<Option<Cookies>>` with `OnceLock<Cookies>` for better performance
- Updated internal session management to use more efficient state handling

### Added
- Comprehensive test suite for axum session extraction
- Integration tests for Redis session store
- Tests for session lifecycle and state management

# 0.3.0
### Added
- `set_expiration` to enable setting a different expiration from the one set in CookieOptions.

### Fixed
- `session.get_all` now returns a `Deserialize`able type `T` instead of `Hashmap<String, T>`

# 0.2.0

### Changed
- Specify `axum` and `redis-store` as optional features

### Removed
- Removed `insert_multiple`

# 0.1.11
- Initial Release
