// Exports ../brand.json to the crate as BRAND_* compile-time variables
// (read through `crate::brand`), so the product name lives in one place.
fn main() {
    println!("cargo:rerun-if-changed=../brand.json");
    let text = std::fs::read_to_string("../brand.json").expect("read ../brand.json");
    let brand: serde_json::Value = serde_json::from_str(&text).expect("parse ../brand.json");
    for (key, var) in [
        ("name", "BRAND_NAME"),
        ("slug", "BRAND_SLUG"),
        ("envPrefix", "BRAND_ENV_PREFIX"),
    ] {
        let value = brand[key].as_str().unwrap_or_else(|| panic!("brand.json: missing \"{key}\""));
        println!("cargo:rustc-env={var}={value}");
    }
    tauri_build::build()
}
