use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub port: u16,
    pub max_body_bytes: usize,
    pub max_instances: u32,
    pub default_time_limit_ms: u64,
    pub default_restarts: u32,
}

impl AppConfig {
    pub fn from_env() -> Self {
        Self {
            port: read_env("PORT", 8080),
            max_body_bytes: read_env("MAX_BODY_BYTES", 5_242_880),
            max_instances: read_env("MAX_INSTANCES", 5_000),
            default_time_limit_ms: read_env("DEFAULT_TIME_LIMIT_MS", 1_200),
            default_restarts: read_env("DEFAULT_RESTARTS", 7),
        }
    }
}

fn read_env<T>(key: &str, default: T) -> T
where
    T: FromStr,
{
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<T>().ok())
        .unwrap_or(default)
}
