use thirdpass_core::extension::FromLib;

fn main() {
    let mut extension = thirdpass_rs_lib::RsExtension::new();
    thirdpass_core::extension::run_command(&mut extension).unwrap();
}
