use tokio::sync::OnceCell;

pub(crate) static INSERT_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static UPDATE_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static UPDATE_MANY_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static INSERT_WITH_RENAME_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static UPDATE_WITH_RENAME_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();

pub(crate) static INSERT_SCRIPT: &str = r#"
    local key = KEYS[1]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_seconds = tonumber(ARGV[3])
    local field_seconds_num = tonumber(ARGV[4])

    local inserted = redis.call('HSETNX', key, field, value)
    if inserted == 1 then
        local pttl = redis.call('PTTL', key)
        if pttl == -1 or (key_seconds * 1000) > pttl then
            if key_seconds and key_seconds > 0 then
                redis.call('EXPIRE', key, key_seconds)
            end
        end
        if field_seconds_num and field_seconds_num > 0 then
            redis.call('HEXPIRE', key, field_seconds_num, 'FIELDS', 1, field)
        end
    end

    return redis.call('TTL', key)
"#;

pub(crate) static UPDATE_SCRIPT: &str = r#"
    local key = KEYS[1]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_seconds = tonumber(ARGV[3])
    local field_seconds_num = tonumber(ARGV[4])

    redis.call('HSET', key, field, value)

    local pttl = redis.call('PTTL', key)
    if pttl == -1 or (key_seconds * 1000) > pttl then
        if key_seconds and key_seconds > 0 then
            redis.call('EXPIRE', key, key_seconds)
        end
    end

    if field_seconds_num and field_seconds_num > 0 then
        redis.call('HEXPIRE', key, field_seconds_num, 'FIELDS', 1, field)
    end

    return redis.call('TTL', key)
"#;

pub(crate) static UPDATE_MANY_SCRIPT: &str = r#"
    -- KEYS[1]: session key
    -- ARGV[1...N]: A flattened list of field-value-field_expiry_seconds triples.

    local key = KEYS[1]

    if (#ARGV % 3) ~= 0 then
        return redis.error_reply("ARGV must be field,value,expiry triples")
    end

    local hset_args = {key}
    local max_field_seconds = 0
    local has_persistent_field = false

    for i = 1, #ARGV, 3 do
        table.insert(hset_args, ARGV[i])     -- field
        table.insert(hset_args, ARGV[i + 1]) -- value

        if not has_persistent_field then
            local field_seconds = tonumber(ARGV[i + 2])
            if field_seconds and field_seconds < 0 then
                has_persistent_field = true
            elseif field_seconds and field_seconds > max_field_seconds then
                max_field_seconds = field_seconds
            end
        end
    end

    redis.call('HSET', unpack(hset_args))

    if has_persistent_field then
        redis.call('PERSIST', key)
    elseif max_field_seconds > 0 then
        local pttl = redis.call('PTTL', key)
        if pttl == -1 or (max_field_seconds * 1000) > pttl then
            redis.call('EXPIRE', key, max_field_seconds)
        end
    end

    for i = 1, #ARGV, 3 do
        local field = ARGV[i]
        local field_seconds = tonumber(ARGV[i + 2])
        if field_seconds and field_seconds > 0 then
            redis.call('HEXPIRE', key, field_seconds, 'FIELDS', 1, field)
        end
    end

    return redis.call('TTL', key)
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
        local pttl = redis.call('PTTL', new_key)
        if pttl == -1 or (key_seconds * 1000) > pttl then
            if key_seconds and key_seconds > 0 then
                redis.call('EXPIRE', new_key, key_seconds)
            end
        end
        if field_seconds_num and field_seconds_num > 0 then
            redis.call('HEXPIRE', new_key, field_seconds_num, 'FIELDS', 1, field)
        end
    end

    return redis.call('TTL', new_key)
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

    local pttl = redis.call('PTTL', new_key)
    if pttl == -1 or (key_seconds * 1000) > pttl then
        if key_seconds and key_seconds > 0 then
            redis.call('EXPIRE', new_key, key_seconds)
        end
    end

    if field_seconds_num and field_seconds_num > 0 then
        redis.call('HEXPIRE', new_key, field_seconds_num, 'FIELDS', 1, field)
    end

    return redis.call('TTL', new_key)
"#;
