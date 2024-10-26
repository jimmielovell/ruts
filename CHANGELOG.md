# 0.3.0
- added `set_expiration` to enable setting a different expiration from the one set in CookieOptions.
- `session.get_all` now returns a `Deserialize`able type `T` instead of `Hashmap<String, T>`

# 0.2.0
- specify `axum` and `redis-store` as optional features
- removed `insert_multiple`

# 0.1.11
- Initial Release
