//! Product identity, generated from `brand.json` at build time (see `build.rs`).
//! Use these instead of spelling the product name in code.

/// Display name, e.g. in dialogs and error messages.
pub const NAME: &str = env!("BRAND_NAME");
/// Lowercase identifier for file names, temp prefixes, and the data directory.
pub const SLUG: &str = env!("BRAND_SLUG");
/// Prefix for environment variables such as `<PREFIX>_DATA_DIR`.
pub const ENV_PREFIX: &str = env!("BRAND_ENV_PREFIX");

/// `<PREFIX>_<suffix>` environment variable name.
pub fn env_var(suffix: &str) -> String {
    format!("{ENV_PREFIX}_{suffix}")
}
