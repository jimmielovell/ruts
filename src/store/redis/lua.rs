use tokio::sync::OnceCell;

pub(crate) static INSERT_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static UPDATE_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static UPDATE_MANY_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static INSERT_WITH_RENAME_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static UPDATE_WITH_RENAME_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static REMOVE_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();

pub(crate) static INSERT_SCRIPT: &str = r#"
    local key = KEYS[1]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_ttl_secs = tonumber(ARGV[3])
    local field_ttl_secs = tonumber(ARGV[4])

    local key_existed = redis.call('EXISTS', key)
    local inserted = redis.call('HSETNX', key, field, value)

    local field_is_persistent = false
    if field_ttl_secs and field_ttl_secs == -1 then
        field_is_persistent = true
    elseif key_ttl_secs and key_ttl_secs == -1 then
        field_is_persistent = true
    end

    if inserted == 1 then
        if field_is_persistent then
            redis.call('PERSIST', key)
            return -1
        end

        if field_ttl_secs and field_ttl_secs > 0 then
            redis.call('HEXPIRE', key, field_ttl_secs, 'FIELDS', 1, field)
        end

        if key_existed == 0 then
            if key_ttl_secs and key_ttl_secs == -2 then
                return -1
            end

            redis.call('EXPIRE', key, key_ttl_secs)
            return key_ttl_secs
        end

        local ttl = redis.call('TTL', key)

        if key_ttl_secs and key_ttl_secs > 0 then
            if ttl == -2 then
                redis.call('EXPIRE', key, key_ttl_secs)
                return key_ttl_secs
            elseif ttl == -1 then
                return -1
            elseif key_ttl_secs > ttl then
                redis.call('EXPIRE', key, key_ttl_secs)
                return key_ttl_secs
            else
                return ttl
            end
        else
            return ttl
        end
    end

    return -2
"#;

pub(crate) static UPDATE_SCRIPT: &str = r#"
    local key = KEYS[1]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_ttl_secs = tonumber(ARGV[3])
    local field_ttl_secs = tonumber(ARGV[4])

    local key_existed = redis.call('EXISTS', key)
    redis.call('HSET', key, field, value)

    local field_is_persistent = false
    if field_ttl_secs and field_ttl_secs == -1 then
        field_is_persistent = true
    elseif key_ttl_secs and key_ttl_secs == -1 then
        field_is_persistent = true
    end

    if field_is_persistent then
        redis.call('PERSIST', key)
        return -1
    end

    if field_ttl_secs and field_ttl_secs > 0 then
        redis.call('HEXPIRE', key, field_ttl_secs, 'FIELDS', 1, field)
    end

    if key_existed == 0 then
        if key_ttl_secs and key_ttl_secs == -2 then
            return -1
        end

        redis.call('EXPIRE', key, key_ttl_secs)
        return key_ttl_secs
    end

    local ttl = redis.call('TTL', key)

    if key_ttl_secs and key_ttl_secs > 0 then
        if ttl == -2 then
            redis.call('EXPIRE', key, key_ttl_secs)
            return key_ttl_secs
        elseif ttl == -1 then
            return -1
        elseif key_ttl_secs > ttl then
            redis.call('EXPIRE', key, key_ttl_secs)
            return key_ttl_secs
        else
            return ttl
        end
    else
        return ttl
    end
"#;

pub(crate) static UPDATE_MANY_SCRIPT: &str = r#"
    local key = KEYS[1]

    if (#ARGV % 3) ~= 0 then
        return redis.error_reply("ARGV must be field,value,expiry triples")
    end

    local hset_args = {key}
    local max_field_ttl_secs = 0
    local has_persistent_field = false

    for i = 1, #ARGV, 3 do
        local field = ARGV[i]
        local field_ttl_secs = tonumber(ARGV[i + 2])

        table.insert(hset_args, field)          -- field
        table.insert(hset_args, ARGV[i + 1])    -- value

        if not has_persistent_field then
            if field_ttl_secs and field_ttl_secs == -1 then
                has_persistent_field = true
            elseif field_ttl_secs and field_ttl_secs > max_field_ttl_secs then
                max_field_ttl_secs = field_ttl_secs
            end
        end
    end

    local key_existed = redis.call('EXISTS', key)
    local updated = redis.call('HSET', unpack(hset_args))

    for i = 1, #ARGV, 3 do
        local field = ARGV[i]
        local field_ttl_secs = tonumber(ARGV[i + 2])
        if field_ttl_secs and field_ttl_secs > 0 then
            redis.call('HEXPIRE', key, field_ttl_secs, 'FIELDS', 1, field)
        end
    end

    if has_persistent_field then
        redis.call('PERSIST', key)
        return -1
    end

    if key_existed == 0 then
        if key_ttl_secs and key_ttl_secs == -2 then
            return -1
        end

        redis.call('EXPIRE', key, key_ttl_secs)
        return key_ttl_secs
    end

    local ttl = redis.call('TTL', key)

    if max_field_ttl_secs and max_field_ttl_secs > 0 then
        if ttl == -2 then
            redis.call('EXPIRE', key, max_field_ttl_secs)
            return max_field_ttl_secs
        elseif ttl == -1 then
            return -1
        elseif max_field_ttl_secs > ttl then
            redis.call('EXPIRE', key, max_field_ttl_secs)
            return max_field_ttl_secs
        else
            return ttl
        end
    else
        return ttl
    end
"#;

pub(crate) static INSERT_WITH_RENAME_SCRIPT: &str = r#"
    local old_key = KEYS[1]
    local new_key = KEYS[2]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_ttl_secs = tonumber(ARGV[3])
    local field_ttl_secs = tonumber(ARGV[4])

    local key_existed = false
    if redis.call('EXISTS', old_key) == 1 then
        key_existed = true
        redis.call('RENAMENX', old_key, new_key)
    end

    local inserted = redis.call('HSETNX', new_key, field, value)

    local field_is_persistent = false
    if field_ttl_secs and field_ttl_secs == -1 then
        field_is_persistent = true
    elseif key_ttl_secs and key_ttl_secs == -1 then
        field_is_persistent = true
    end

    if inserted == 1 then
        if field_is_persistent then
            redis.call('PERSIST', new_key)
            return -1
        end

        if field_ttl_secs and field_ttl_secs > 0 then
            redis.call('HEXPIRE', new_key, field_ttl_secs, 'FIELDS', 1, field)
        end

        if key_existed == 0 then
            if key_ttl_secs and key_ttl_secs == -2 then
                return -1
            end

            redis.call('EXPIRE', new_key, key_ttl_secs)
            return key_ttl_secs
        end

        local ttl = redis.call('TTL', new_key)

        if key_ttl_secs and key_ttl_secs > 0 then
            if ttl == -2 then
                redis.call('EXPIRE', new_key, key_ttl_secs)
                return key_ttl_secs
            elseif ttl == -1 then
                return -1
            elseif key_ttl_secs > ttl then
                redis.call('EXPIRE', new_key, key_ttl_secs)
                return key_ttl_secs
            else
                return ttl
            end
        else
            return ttl
        end
    end

    return -2
"#;

pub(crate) static UPDATE_WITH_RENAME_SCRIPT: &str = r#"
    local old_key = KEYS[1]
    local new_key = KEYS[2]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_ttl_secs = tonumber(ARGV[3])
    local field_ttl_secs = tonumber(ARGV[4])

    local key_existed = false
    if redis.call('EXISTS', old_key) == 1 then
        key_existed = true
        redis.call('RENAMENX', old_key, new_key)
    end

    redis.call('HSET', new_key, field, value)

    local field_is_persistent = false
    if field_ttl_secs and field_ttl_secs == -1 then
        field_is_persistent = true
    elseif key_ttl_secs and key_ttl_secs == -1 then
        field_is_persistent = true
    end

    if field_is_persistent then
        redis.call('PERSIST', new_key)
        return -1
    end

    if field_ttl_secs and field_ttl_secs > 0 then
        redis.call('HEXPIRE', new_key, field_ttl_secs, 'FIELDS', 1, field)
    end

    if key_existed == 0 then
        if key_ttl_secs and key_ttl_secs == -2 then
            return -1
        end

        redis.call('EXPIRE', new_key, key_ttl_secs)
        return key_ttl_secs
    end

    local ttl = redis.call('TTL', new_key)

    if key_ttl_secs and key_ttl_secs > 0 then
        if ttl == -2 then
            redis.call('EXPIRE', new_key, key_ttl_secs)
            return key_ttl_secs
        elseif ttl == -1 then
            return -1
        elseif key_ttl_secs > ttl then
            redis.call('EXPIRE', new_key, key_ttl_secs)
            return key_ttl_secs
        else
            return ttl
        end
    else
        return ttl
    end
"#;

pub(crate) static REMOVE_SCRIPT: &str = r#"
    local removed = redis.call("HDEL", KEYS[1], ARGV[1])

    if removed > 0 then
        return redis.call("TTL", KEYS[1])
    end

    return -2
"#;
