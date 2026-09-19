fn main() {
    #[cfg(feature = "esp32")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        embuild::espidf::sysenv::output();
    }
    for key in [
        "NANACOIN_WIFI_SSID",
        "NANACOIN_WIFI_PASSWORD",
        "NANACOIN_ORIGINS",
        "NANACOIN_NTP_SERVER",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    println!("cargo:rerun-if-changed=certs/server.crt");
    println!("cargo:rerun-if-changed=certs/server.key");
}
