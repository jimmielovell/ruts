use tokio::sync::OnceCell;

pub(crate) static INSERT_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static UPDATE_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static INSERT_WITH_RENAME_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static UPDATE_WITH_RENAME_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();

pub(crate) static RENAME_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();

pub(crate) static INSERT_SCRIPT: &str = r#"
    local key = KEYS[1]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_seconds = tonumber(ARGV[3])
    local field_seconds = ARGV[4]

    local inserted = redis.call('HSETNX', key, field, value)
    if inserted == 1 then
        redis.call('EXPIRE', key, key_seconds)
        if field_seconds ~= '' then
            redis.call('HEXPIRE', key, tonumber(field_seconds), 'FIELDS', 1, field)
        end
    end
    return inserted
"#;

pub(crate) static UPDATE_SCRIPT: &str = r#"
    local key = KEYS[1]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_seconds = tonumber(ARGV[3])
    local field_seconds = ARGV[4]

    local updated = redis.call('HSET', key, field, value)
    if updated == 1 then
        redis.call('EXPIRE', key, key_seconds)
        if field_seconds ~= '' then
            redis.call('HEXPIRE', key, tonumber(field_seconds), 'FIELDS', 1, field)
        end
    end
    return updated
"#;

pub(crate) static INSERT_WITH_RENAME_SCRIPT: &str = r#"
    local old_key = KEYS[1]
    local new_key = KEYS[2]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_seconds = tonumber(ARGV[3])
    local field_seconds = ARGV[4]

    local exists = redis.call('EXISTS', old_key)
    if exists == 0 then
        return 0
    end

    local new_exists = redis.call('EXISTS', new_key)
    if new_exists == 1 then
        return 0
    end

    local inserted = redis.call('HSETNX', old_key, field, value)
    if inserted == 1 then
        redis.call('RENAMENX', old_key, new_key)
        redis.call('EXPIRE', new_key, key_seconds)
        if field_seconds ~= '' then
            redis.call('HEXPIRE', new_key, tonumber(field_seconds), 'FIELDS', 1, field)
        end
    end
    return inserted
"#;

pub(crate) static UPDATE_WITH_RENAME_SCRIPT: &str = r#"
    local old_key = KEYS[1]
    local new_key = KEYS[2]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_seconds = tonumber(ARGV[3])
    local field_seconds = ARGV[4]

    local exists = redis.call('EXISTS', old_key)
    if exists == 0 then
        return 0
    end

    local new_exists = redis.call('EXISTS', new_key)
    if new_exists == 1 then
        return 0
    end

    -- Update the field
    local updated = redis.call('HSET', old_key, field, value)
    if updated == 1 then
        redis.call('RENAMENX', old_key, new_key)
        redis.call('EXPIRE', new_key, key_seconds)
        if field_seconds ~= '' then
            redis.call('HEXPIRE', new_key, tonumber(field_seconds), 'FIELDS', 1, field)
        end
    end
    return updated
"#;

pub(crate) const RENAME_SCRIPT: &str = r#"
    local old_key = KEYS[1]
    local new_key = KEYS[2]
    local seconds = tonumber(ARGV[1])

    local renamed = redis.call('RENAMENX', old_key, new_key)
    if renamed == 1 then
        redis.call('EXPIRE', new_key, seconds)
    end
    return renamed
"#;
