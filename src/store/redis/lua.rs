use tokio::sync::OnceCell;

pub static INSERT_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();
pub static UPDATE_SCRIPT_HASH: OnceCell<String> = OnceCell::const_new();

pub static INSERT_SCRIPT: &str = r#"
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

pub static UPDATE_SCRIPT: &str = r#"
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
