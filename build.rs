fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        use std::{env, fs, path::PathBuf, process::Command};
        println!("cargo:rerun-if-changed=assets/keyisle.ico");
        println!("cargo:rerun-if-changed=assets/keyisle.rc");
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
        let version = env::var("CARGO_PKG_VERSION").expect("version");
        let numbers = format!("{},0", version.split('.').collect::<Vec<_>>().join(","));
        let resource = fs::read_to_string("assets/keyisle.rc")
            .expect("read resource")
            .replace("@VERSION_COMMAS@", &numbers)
            .replace("@VERSION_DOTS@", &version);
        let source = out.join("keyisle.rc");
        let compiled = out.join("keyisle.res");
        fs::write(&source, resource).expect("write resource");
        let status = Command::new(
            env::var("KEYISLE_RESOURCE_COMPILER").unwrap_or_else(|_| "llvm-rc".into()),
        )
        .arg("/fo")
        .arg(&compiled)
        .arg(&source)
        .status()
        .expect("run llvm-rc");
        assert!(status.success(), "compile Keyisle Windows resources");
        println!(
            "cargo:rustc-link-arg-bin=windows-password-manager={}",
            compiled.display()
        );
        slint_build::compile_with_config(
            "ui/main.slint",
            slint_build::CompilerConfiguration::new().with_style("fluent".into()),
        )
        .expect("compile Slint UI");
    }
}
