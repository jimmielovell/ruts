use tokio::sync::OnceCell;

pub(crate) static SET_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static SET_MULTIPLE_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static SET_AND_RENAME_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub(crate) static REMOVE_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();

pub(crate) static SET_SCRIPT: &str = r#"
    local key = KEYS[1]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_ttl_secs = tonumber(ARGV[3])
    local field_ttl_secs = tonumber(ARGV[4])

    local key_existed = redis.call('EXISTS', key)
    local effective_key_ttl = key_ttl_secs

    if field_ttl_secs then
        if field_ttl_secs == -1 then
            effective_key_ttl = -1
        elseif field_ttl_secs > 0 then
            if effective_key_ttl == nil then
                effective_key_ttl = field_ttl_secs
            elseif effective_key_ttl ~= -1 and field_ttl_secs > effective_key_ttl then
                effective_key_ttl = field_ttl_secs
            end
        end
    end

    if field_ttl_secs and field_ttl_secs == 0 then
        redis.call('HDEL', key, field)
    else
        redis.call('HSET', key, field, value)
        if field_ttl_secs and field_ttl_secs > 0 then
            redis.call('HEXPIRE', key, field_ttl_secs, 'FIELDS', 1, field)
        elseif field_ttl_secs and field_ttl_secs == -1 then
            redis.call('HPERSIST', key, 'FIELDS', 1, field)
        end
    end

    if redis.call('EXISTS', key) == 0 then
        return -2
    end

    if effective_key_ttl and effective_key_ttl == -1 then
        redis.call('PERSIST', key)
        return -1
    end

    if effective_key_ttl and effective_key_ttl > 0 then
        if key_existed == 0 then
            redis.call('EXPIRE', key, effective_key_ttl)
            return effective_key_ttl
        else
            local current_ttl = redis.call('TTL', key)
            if current_ttl == -1 then
                return -1
            elseif effective_key_ttl > current_ttl then
                redis.call('EXPIRE', key, effective_key_ttl)
                return effective_key_ttl
            else
                return current_ttl
            end
        end
    end

    return redis.call('TTL', key)
"#;

pub(crate) static SET_MULTIPLE_SCRIPT: &str = r#"
    local key = KEYS[1]

    if (#ARGV % 3) ~= 0 then
        return redis.error_reply("ARGV must be field,value,expiry triples")
    end

    local key_existed = redis.call('EXISTS', key)
    local max_finite_ttl = 0
    local has_persistent_field = false

    for i = 1, #ARGV, 3 do
        local f_ttl = tonumber(ARGV[i + 2])
        if f_ttl then
            if f_ttl == -1 then
                has_persistent_field = true
            elseif f_ttl > max_finite_ttl then
                max_finite_ttl = f_ttl
            end
        end
    end

    local hset_args = {key}
    for i = 1, #ARGV, 3 do
        table.insert(hset_args, ARGV[i])     -- field
        table.insert(hset_args, ARGV[i + 1]) -- value
    end
    redis.call('HSET', unpack(hset_args))

    for i = 1, #ARGV, 3 do
        local field = ARGV[i]
        local f_ttl = tonumber(ARGV[i + 2])
        if f_ttl then
            if f_ttl > 0 then
                redis.call('HEXPIRE', key, f_ttl, 'FIELDS', 1, field)
            elseif f_ttl == -1 then
                redis.call('HPERSIST', key, 'FIELDS', 1, field)
            end
        end
    end

    if has_persistent_field then
        redis.call('PERSIST', key)
        return -1
    end

    if max_finite_ttl > 0 then
        if key_existed == 0 then
            redis.call('EXPIRE', key, max_finite_ttl)
            return max_finite_ttl
        else
            local current_ttl = redis.call('TTL', key)
            if current_ttl == -1 then
                return -1
            elseif max_finite_ttl > current_ttl then
                redis.call('EXPIRE', key, max_finite_ttl)
                return max_finite_ttl
            else
                return current_ttl
            end
        end
    end

    return redis.call('TTL', key)
"#;

pub(crate) static SET_AND_RENAME_SCRIPT: &str = r#"
    local old_key = KEYS[1]
    local new_key = KEYS[2]
    local field = ARGV[1]
    local value = ARGV[2]
    local key_ttl_secs = tonumber(ARGV[3])
    local field_ttl_secs = tonumber(ARGV[4])

    local effective_key_ttl = key_ttl_secs
    if field_ttl_secs then
        if field_ttl_secs == -1 then
            effective_key_ttl = -1
        elseif field_ttl_secs > 0 then
            if effective_key_ttl == nil then
                effective_key_ttl = field_ttl_secs
            elseif effective_key_ttl ~= -1 and field_ttl_secs > effective_key_ttl then
                effective_key_ttl = field_ttl_secs
            end
        end
    end

    if redis.call('EXISTS', new_key) == 1 then
        return redis.error_reply("Target session ID already exists")
    end

    local key_existed_before_rename = 0
    if redis.call('EXISTS', old_key) == 1 then
        key_existed_before_rename = 1
        local success = redis.call('RENAMENX', old_key, new_key)
        if success == 0 then
             return redis.error_reply("Target session ID collision during RENAME")
        end
    end

    if field_ttl_secs and field_ttl_secs == 0 then
        redis.call('HDEL', new_key, field)
    else
        redis.call('HSET', new_key, field, value)
        if field_ttl_secs and field_ttl_secs > 0 then
            redis.call('HEXPIRE', new_key, field_ttl_secs, 'FIELDS', 1, field)
        elseif field_ttl_secs and field_ttl_secs == -1 then
            redis.call('HPERSIST', new_key, 'FIELDS', 1, field)
        end
    end

    if redis.call('EXISTS', new_key) == 0 then
        return -2
    end

    if effective_key_ttl and effective_key_ttl == -1 then
        redis.call('PERSIST', new_key)
        return -1
    end

    if effective_key_ttl and effective_key_ttl > 0 then
        if key_existed_before_rename == 0 then
            redis.call('EXPIRE', new_key, effective_key_ttl)
            return effective_key_ttl
        else
            local current_ttl = redis.call('TTL', new_key)
            if current_ttl == -1 then
                return -1
            elseif effective_key_ttl > current_ttl then
                redis.call('EXPIRE', new_key, effective_key_ttl)
                return effective_key_ttl
            else
                return current_ttl
            end
        end
    end

    return redis.call('TTL', new_key)
"#;

pub(crate) static REMOVE_SCRIPT: &str = r#"
    local removed = redis.call("HDEL", KEYS[1], ARGV[1])

    if removed > 0 then
        return redis.call("TTL", KEYS[1])
    end

    return -2
"#;
