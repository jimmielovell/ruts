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
    local field_seconds_num = tonumber(ARGV[4])

    local inserted = redis.call('HSETNX', key, field, value)
    if inserted == 1 then
        if key_seconds and key_seconds > 0 then
            redis.call('EXPIRE', key, key_seconds)
        end
        if field_seconds_num and field_seconds_num > 0 then
            redis.call('HEXPIRE', key, field_seconds_num, 'FIELDS', 1, field)
        end
    end

    return inserted
"#;

pub(crate) static UPDATE_SCRIPT: &str = r#"
    local key = KEYS[1]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_seconds = tonumber(ARGV[3])
    local field_seconds_num = tonumber(ARGV[4])

    redis.call('HSET', key, field, value)
    
    if key_seconds and key_seconds > 0 then
        redis.call('EXPIRE', key, key_seconds)
    end
    
    if field_seconds_num and field_seconds_num > 0 then
        redis.call('HEXPIRE', key, field_seconds_num, 'FIELDS', 1, field)
    end

    return 1
"#;

pub(crate) static INSERT_WITH_RENAME_SCRIPT: &str = r#"
    local old_key = KEYS[1]
    local new_key = KEYS[2]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_seconds = tonumber(ARGV[3])
    local field_seconds_num = tonumber(ARGV[4])

    -- Only rename if old key exists
    if redis.call('EXISTS', old_key) == 1 then
        if redis.call('RENAMENX', old_key, new_key) == 0 then
            return 0  -- Rename failed (new_key exists)
        end
    end

    local inserted = redis.call('HSETNX', new_key, field, value)
    if inserted == 1 then
        if key_seconds and key_seconds > 0 then
            redis.call('EXPIRE', new_key, key_seconds)
        end
        if field_seconds_num and field_seconds_num > 0 then
            redis.call('HEXPIRE', new_key, field_seconds_num, 'FIELDS', 1, field)
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
    local field_seconds_num = tonumber(ARGV[4])

    if redis.call('EXISTS', old_key) == 1 then
        if redis.call('RENAMENX', old_key, new_key) == 0 then
            return 0
        end
    end

    redis.call('HSET', new_key, field, value)
    
    if key_seconds and key_seconds > 0 then
        redis.call('EXPIRE', new_key, key_seconds)
    end
    
    if field_seconds_num and field_seconds_num > 0 then
        redis.call('HEXPIRE', new_key, field_seconds_num, 'FIELDS', 1, field)
    end

    return 1
"#;

pub(crate) const RENAME_SCRIPT: &str = r#"
    local old_key = KEYS[1]
    local new_key = KEYS[2]
    local seconds = tonumber(ARGV[1])

    local renamed = redis.call('RENAMENX', old_key, new_key)
    if renamed == 1 and seconds and seconds > 0 then
        redis.call('EXPIRE', new_key, seconds)
    end

    return renamed
"#;
