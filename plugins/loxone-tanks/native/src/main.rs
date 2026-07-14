fn main() {
    if let Err(error) = venus_loxone_tanks_plugin::service::run() {
        eprintln!("venus-loxone-tanks: {error}");
        std::process::exit(1);
    }
}
