use thirdpass_lib::extension::FromLib;
use thirdpass_rs_lib;

fn main() {
    let mut extension = thirdpass_rs_lib::RsExtension::new();
    thirdpass_lib::extension::commands::run(&mut extension).unwrap();
}
