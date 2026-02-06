use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub port: u16,
    pub max_body_bytes: usize,
    pub max_instances: u32,
    pub default_time_limit_ms: u64,
    pub default_restarts: u32,
    pub max_concurrent_optimize: usize,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let cpu_default = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(1);

        Self {
            port: read_env("PORT", 8088),
            max_body_bytes: read_env("MAX_BODY_BYTES", 5_242_880),
            max_instances: read_env("MAX_INSTANCES", 5_000),
            default_time_limit_ms: read_env("DEFAULT_TIME_LIMIT_MS", 2_000),
            default_restarts: read_env("DEFAULT_RESTARTS", 10),
            max_concurrent_optimize: read_env("MAX_CONCURRENT_OPTIMIZE", cpu_default),
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
