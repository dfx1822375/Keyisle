fn main() {
    let dir = sqlcipher_src::source_dir();
    let mut build = cc::Build::new();
    build
        .include(dir)
        .include(dir.join(sqlcipher_src::LIBTOMCRYPT_INCLUDE_DIR))
        .file(dir.join(sqlcipher_src::SOURCE_FILE))
        .define("SQLITE_HAS_CODEC", None)
        .define("SQLCIPHER_CRYPTO_LIBTOMCRYPT", None)
        .define("SQLITE_EXTRA_INIT", "sqlcipher_extra_init")
        .define("SQLITE_EXTRA_SHUTDOWN", "sqlcipher_extra_shutdown")
        .define("SQLITE_TEMP_STORE", "3")
        .define("SQLITE_THREADSAFE", "1")
        .define("SQLITE_ENABLE_COLUMN_METADATA", None)
        .define("SQLITE_ENABLE_API_ARMOR", None)
        .define("SQLITE_SECURE_DELETE", None)
        .define("SQLITE_OMIT_LOAD_EXTENSION", None)
        .define("SQLITE_OMIT_DEPRECATED", None)
        .define("SQLITE_DQS", "0")
        .define("SQLITE_DEFAULT_MEMSTATUS", "0")
        .define("LTC_SOURCE", None)
        .define("LTC_NO_PK", None)
        .define("LTC_NO_PKCS", None)
        .define("LTC_PKCS_5", None)
        .define("LTC_NO_ASM", None)
        .define("ARGTYPE", "4")
        .warnings(false);
    for source in sqlcipher_src::LIBTOMCRYPT_SOURCES {
        build.file(dir.join(source));
    }
    build.compile("sqlcipher");
    std::fs::copy(
        dir.join(sqlcipher_src::HEADER_FILE),
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("sqlite3.h"),
    )
    .expect("copy SQLCipher header");
    println!("cargo:rustc-link-lib=advapi32");
    println!("cargo:rerun-if-changed=build.rs");
}
