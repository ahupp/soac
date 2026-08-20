fn main() {
    for name in [
        "SOAC_TY_RUFF_REVISION",
        "SOAC_TY_CHECKER_FINGERPRINT",
        "SOAC_TY_EXPORTER_FINGERPRINT",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
        let value = std::env::var(name).unwrap_or_else(|_| {
            panic!("build the matched, verified checker using `just ty`, not bare cargo")
        });
        let valid_length = match name {
            "SOAC_TY_RUFF_REVISION" => matches!(value.len(), 40 | 64),
            _ => value.len() == 64,
        };
        assert!(
            valid_length
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid {name} from verified checker build wrapper"
        );
        println!("cargo:rustc-env={name}={value}");
    }
}
