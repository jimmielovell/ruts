use fred::types::scripts::Script;
use std::sync::LazyLock;

pub(crate) static SET_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::from_lua(
        r#"
        local key = KEYS[1]
        local field = ARGV[1]
        local value = ARGV[2]
        local field_ttl = tonumber(ARGV[3])

        redis.call('HSET', key, field, value)
        redis.call('HEXPIRE', key, field_ttl, 'FIELDS', 1, field)

        return 1
        "#,
    )
});

pub(crate) static SET_MULTIPLE_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::from_lua(
        r#"
        local key = KEYS[1]

        if (#ARGV % 3) ~= 0 then
            return redis.error_reply("ARGV must be field,value,ttl triples")
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
            if not f_ttl or f_ttl <= 0 then
                return redis.error_reply("ttl must be strictly positive (> 0)")
            end
            redis.call('HEXPIRE', key, f_ttl, 'FIELDS', 1, field)
        end

        return 1
    "#,
    )
});

pub(crate) static SET_AND_RENAME_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::from_lua(
        r#"
        local old_key = KEYS[1]
        local new_key = KEYS[2]
        local field = ARGV[1]
        local value = ARGV[2]
        local field_ttl = tonumber(ARGV[3])

        if old_key ~= new_key and redis.call('EXISTS', new_key) == 1 then
            return 0
        end

        if old_key ~= new_key and redis.call('EXISTS', old_key) == 1 then
            redis.call('RENAME', old_key, new_key)
        end

        redis.call('HSET', new_key, field, value)
        redis.call('HEXPIRE', new_key, field_ttl, 'FIELDS', 1, field)

        return 1
    "#,
    )
});

pub(crate) static RENAME_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::from_lua(
        r#"
        local old_key = KEYS[1]
        local new_key = KEYS[2]

        if old_key == new_key then
            if redis.call('EXISTS', old_key) == 1 then return 1 else return 0 end
        end

        if redis.call('EXISTS', old_key) == 0 then
            return 0
        end

        -- Fixation guard: renaming onto an existing session id is an error.
        if redis.call('EXISTS', new_key) == 1 then
            return 0
        end

        redis.call('RENAME', old_key, new_key)
        return 1
    "#,
    )
});

pub(crate) static EXPIRE_FIELD_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::from_lua(
        r#"
        local key = KEYS[1]
        local field = ARGV[1]
        local ttl = tonumber(ARGV[2])

        local res = redis.call('HEXPIRE', key, ttl, 'FIELDS', 1, field)

        if type(res) == "table" then
            if tonumber(res[1]) > 0 then return 1 else return 0 end
        elseif tonumber(res) and tonumber(res) > 0 then
            return 1
        end

        return 0
    "#,
    )
});
