fn main() {
    if let Err(error) = venus_rathole_plugin::service::run() {
        eprintln!("venus-rathole: {error}");
        std::process::exit(1);
    }
}
